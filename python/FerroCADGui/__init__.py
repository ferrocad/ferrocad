"""The FerroCAD-only ``Gui`` API (the extensions upstream FreeCADGui lacks).

A mod that imports ``FerroCADGui`` declares a FerroCAD dependency. Nothing here
is aliased onto ``FreeCADGui``: the upstream namespace keeps only the
upstream-shaped command registry, selection snapshot and dialog helpers. See
``docs/developer/architecture/api-namespaces.md``.

What lives here:

* the **mod provider registry** — ``registerProvider`` / ``providers`` /
  ``activeProvider`` / ``setActiveProvider`` / ``refreshProviders``: a mod
  declares which *feature* a backend provides, exactly one backend per feature
  is active, and switching runs the outgoing ``deactivate`` and incoming
  ``activate`` callbacks; a later request can run the active backend's
  ``refresh`` callback without switching (see
  ``docs/developer/workbenches/mod-providers.md``);
* the **provider settings schema** — a provider may advertise its knobs in
  ``registerProvider(..., settings=[...])``; ``settings`` returns the active
  backend's rows in the property-pane row shape and ``setSetting`` validates,
  stores and refreshes (see ``docs/project/record/tasks/settings-model.md`` S0);
* the **edition appearance presets** — ``registerAppearancePreset`` /
  ``installAppearancePresets`` / ``resetAppearance``: an edition declares its own
  defaults for the gizmo, axis and grid appearance. The preset is a fallback
  layer under the user's persisted parameter values, and ``resetAppearance``
  restores it (see ``docs/mods/settings-and-parameters.md``);
* the **pull overlay bridge** — ``addOverlay`` / ``removeOverlay`` (Coin);
* the **document-listener slot** — ``addDocumentListener`` /
  ``removeDocumentListener``, with ``onDocumentCreated`` / ``onDocumentActivated``
  / ``onDocumentDeactivated`` / ``onDocumentClosed`` callbacks.
  ``FreeCADGui.addDocumentObserver`` forwards here (the upstream-shaped name),
  and the App observer reports creation/activation/deletion but never
  deactivation — which is why a per-document attachment used to linger until the
  next activation;
* a **read-only view state** (``ViewState.grid``, ``ViewState.projection``),
  filled by the shell for a checkable command's ``IsChecked``.

The command registry, the selection snapshot and the private bridge helpers the
app script calls (``_command_list``, ``_run``, ``_accept``, ``_cancel``) stay on
``FreeCADGui``, because upstream has them.
"""

from __future__ import annotations

import importlib
import json
import re
import sys

import FreeCAD

from FreeCADGui import _error_text

#: Provider manifests: ``feature -> backend -> record``. Both levels keep
#: registration order, which is the deterministic tie-break for equal priorities.
_PROVIDERS: dict = {}

#: ``feature -> backend`` whose ``activate``/``deactivate`` callbacks last ran.
#: A persisted choice alone does not mean the effects were applied: the first
#: explicit switch is what runs them.
_PROVIDER_APPLIED: dict = {}

#: The document the applied provider effects are attached to. Deactivation only
#: runs for that document, so closing an inactive document leaves the active
#: one's attachment alone.
_PROVIDER_DOCUMENT = None

#: Monotonic registration serial (never reused), for the priority tie-break.
_PROVIDER_SERIAL: int = 0

#: ``(feature, backend, hook)`` provider callbacks already reported, so a broken
#: backend does not spam the console on every switch.
_PROVIDER_REPORTED: set = set()

#: The parameter group a mod's provider choice is persisted in.
PROVIDER_PREFERENCES = "User parameter:BaseApp/Preferences/Mods"

#: Setting kinds the editor knows how to edit. A schema may declare another
#: kind: the row is then read-only and the problem is reported once.
_SETTING_KINDS = ("text", "bool", "int", "float", "quantity", "enum", "color")

#: Setting kind -> the property type id a row reports (``properties`` uses the
#: same ids to pick an editor), so one renderer serves both surfaces.
_SETTING_TYPES = {
    "text": "App::PropertyString",
    "bool": "App::PropertyBool",
    "int": "App::PropertyInteger",
    "float": "App::PropertyFloat",
    "quantity": "App::PropertyQuantity",
    "enum": "App::PropertyEnumeration",
    "color": "App::PropertyString",
}

#: A sensible default when a schema entry supplies none. ``enum`` and the
#: unknown kinds are handled separately (first choice / empty text).
_SETTING_DEFAULTS = {
    "text": "",
    "bool": False,
    "int": 0,
    "float": 0.0,
    "quantity": 0.0,
    "color": "#000000",
}

#: ``(feature, backend, setting)`` settings problems already reported.
_SETTING_REPORTED: set = set()

#: Observers registered through :func:`addDocumentListener`, in registration
#: order. Identity is the key: one registration per object, no duplicates.
_DOCUMENT_LISTENERS: list = []

#: ``(id(observer), hook)`` document callbacks already reported, so a broken
#: observer does not spam the console on every document operation.
_OBSERVER_REPORTED: set = set()

#: Lifecycle event -> observer method. The names are FerroCAD's; upstream
#: FreeCADGui has no equivalent slot.
_DOCUMENT_HOOKS = {
    "created": "onDocumentCreated",
    "activated": "onDocumentActivated",
    "deactivated": "onDocumentDeactivated",
    "closed": "onDocumentClosed",
}

_COLOR_RE = re.compile(r"#[0-9a-fA-F]{6}\Z")


# ---------------------------------------------------------------------------
# Document lifecycle observers
#
# App-side document creation and activation already exist, but the facade has
# no GUI document observer and no *deactivation* event. A provider that attaches
# something per document (the Coin grid) therefore kept it until the next
# activation. These observers are fired by the FerroCAD lifecycle wrappers and
# by the shell, which is the host's single path through document creation,
# activation and closing.
# ---------------------------------------------------------------------------


def addDocumentListener(observer) -> None:
    """Register ``observer`` for document lifecycle notifications.

    ``observer`` is any object. For each event, the matching method is called
    when it exists: ``onDocumentCreated(name)``, ``onDocumentActivated(name)``,
    ``onDocumentDeactivated(name)`` and ``onDocumentClosed(name)``. A missing
    method is simply not called, so an observer may implement only the events it
    cares about.

    Registration is by **identity**: adding the same object twice is a no-op.
    An observer that raises is reported once per ``(observer, method)`` and is
    skipped; it never prevents the document operation or the other observers.
    This slot lives on ``FerroCADGui`` because it is a FerroCAD extension: the
    App observer reports creation/activation/deletion without a deactivation
    event, and ``FreeCADGui.addDocumentObserver`` forwards here for the
    upstream-shaped name (see ``docs/developer/architecture/api-namespaces.md``).
    """
    if any(existing is observer for existing in _DOCUMENT_LISTENERS):
        return
    _DOCUMENT_LISTENERS.append(observer)


def removeDocumentListener(observer) -> bool:
    """Unregister ``observer``; returns whether it was registered."""
    for index, existing in enumerate(_DOCUMENT_LISTENERS):
        if existing is observer:
            del _DOCUMENT_LISTENERS[index]
            _OBSERVER_REPORTED.difference_update(
                {key for key in _OBSERVER_REPORTED if key[0] == id(observer)}
            )
            return True
    return False


def _report_observer(observer, hook: str, exc: BaseException) -> None:
    """Report a failing observer callback once per ``(observer, hook)``."""
    key = (id(observer), hook)
    if key in _OBSERVER_REPORTED:
        return
    _OBSERVER_REPORTED.add(key)
    label = getattr(observer, "__name__", None) or type(observer).__name__
    print(
        f"FerroCADGui: document observer {label!r} {hook} raised: "
        f"{_error_text(exc)}",
        file=sys.stderr,
    )


def _document_event(event: str, name: str) -> None:
    """Deliver one lifecycle event to every registered observer (private).

    Called by the App facade (``FreeCAD``) and by ``ferrocad_shell``. The
    callbacks are isolated individually, so one broken observer cannot stop the
    document operation or another observer.

    ``closed`` also drops the ``FreeCADGui`` view cached for that document, when
    the facade is loaded: the view owns a scene graph and may hold an overlay
    registration, so leaving it behind would keep drawing a closed document.
    """
    hook = _DOCUMENT_HOOKS[event]
    for observer in list(_DOCUMENT_LISTENERS):
        callback = getattr(observer, hook, None)
        if not callable(callback):
            continue
        try:
            callback(name)
        except Exception as exc:  # noqa: BLE001 - one observer is not fatal
            _report_observer(observer, hook, exc)
    if event == "closed":
        gui = sys.modules.get("FreeCADGui")
        forget = getattr(gui, "_forget_document", None) if gui is not None else None
        if forget is not None:
            try:
                forget(name)
            except Exception as exc:  # noqa: BLE001 - a stale view is not fatal
                _report_observer(gui, "_forget_document", exc)


# ---------------------------------------------------------------------------
# Mod providers
#
# A mod that replaces a feature's implementation (the semantic and Coin grids
# are the first pair) declares itself from ``InitGui.py`` with
# :func:`registerProvider`. The registry keeps exactly one active backend per
# feature and selects it by calling the outgoing provider's ``deactivate`` and
# the incoming provider's ``activate``. Those callbacks own the implementation
# and its viewport claim, **not** the feature's visibility switch: ``activate``
# also runs on document activation, so writing ``enabled`` there would undo a
# user's "off". A command that changed shared state without switching can ask
# the active backend to re-read it with :func:`refreshProviders`. See
# ``docs/mods/providers.md`` and ``docs/developer/workbenches/mod-providers.md``.
# ---------------------------------------------------------------------------


def _provider_prefs():
    """The parameter group holding one backend id per feature."""
    return FreeCAD.ParamGet(PROVIDER_PREFERENCES).GetGroup("Providers")


def _report_provider(feature, backend, hook: str, exc: BaseException) -> None:
    """Report a failing ``activate``/``deactivate``/``refresh`` once per hook."""
    key = (feature, backend, hook)
    if key in _PROVIDER_REPORTED:
        return
    _PROVIDER_REPORTED.add(key)
    print(
        f"FerroCADGui: provider {backend!r} for {feature!r} {hook} raised: "
        f"{_error_text(exc)}",
        file=sys.stderr,
    )


def _report_setting(feature, backend, name: str, exc: BaseException) -> None:
    """Report a broken schema default/hook once per setting."""
    key = (feature, backend, name)
    if key in _SETTING_REPORTED:
        return
    _SETTING_REPORTED.add(key)
    print(
        f"FerroCADGui: setting {name!r} of provider {backend!r} for {feature!r} "
        f"raised: {_error_text(exc)}",
        file=sys.stderr,
    )


def registerProvider(
    feature: str,
    backend: str,
    label: str,
    command=None,
    priority: int = 0,
    activate=None,
    deactivate=None,
    refresh=None,
    settings=None,
    skin=None,
):
    """Declare that ``backend`` provides ``feature`` (called from ``InitGui.py``).

    ``label`` is the user-facing name the ``Mods_Settings`` dialog shows;
    ``command`` is the command id the backend owns (``None`` when it has none);
    ``priority`` decides the default backend (higher wins, earliest registration
    breaks a tie); ``activate``/``deactivate`` are optional zero-argument
    callbacks that make the backend the drawn one and undo that; ``refresh`` is
    the optional zero-argument callback for "shared state changed, re-read it"
    (see :func:`refreshProviders`). All three callbacks run only when asked —
    never at registration — and their errors are isolated. One exception: if
    this registration takes the selection over (a higher priority, or a saved
    choice), the outgoing backend's ``deactivate`` runs so a mod that drew
    during its own import cannot keep drawing. The incoming backend is still
    activated by the first switch.

    ``settings`` is the optional schema the provider advertises: a list of
    dicts, each with a non-empty ``name`` and a ``kind`` (``text``, ``bool``,
    ``int``, ``float``, ``quantity``, ``enum``, ``color``), plus ``label``,
    ``default``, ``min``/``max``, ``unit``, ``choices`` and optional ``get``/
    ``set`` callables. It is validated here and stored; **no default is written**
    at registration, so importing a mod stays free of side effects. An unknown
    kind is allowed and reported once; its row is read-only. See
    ``docs/project/record/tasks/settings-model.md`` S0.

    ``skin`` is the optional WGSL source text of the material program this
    backend supplies for a capability that hosts one (the manipulator gizmo
    does). It is **not** a setting: it never appears in the settings manifest or
    in the mods dialog, and the host asks for it through :func:`providerSkin`.
    Supplying no skin means the host draws its native material. A reload
    replaces the source like every other field; omitting it clears a previously
    supplied one. It is stored trimmed.

    Registering the same ``(feature, backend)`` again replaces the record (a mod
    reload) without moving it in the registration order. Returns the manifest.
    """
    global _PROVIDER_SERIAL

    if not isinstance(feature, str) or not feature:
        raise ValueError("a provider feature must be a non-empty string")
    if not isinstance(backend, str) or not backend:
        raise ValueError("a provider backend must be a non-empty string")
    if command is not None and not isinstance(command, str):
        raise TypeError("a provider command must be a command id string or None")
    if activate is not None and not callable(activate):
        raise TypeError(f"provider {feature!r}/{backend!r} activate is not callable")
    if deactivate is not None and not callable(deactivate):
        raise TypeError(f"provider {feature!r}/{backend!r} deactivate is not callable")
    if refresh is not None and not callable(refresh):
        raise TypeError(f"provider {feature!r}/{backend!r} refresh is not callable")
    schema = _validate_settings(feature, backend, settings)
    if skin is not None and not isinstance(skin, str):
        raise TypeError(f"provider {feature!r}/{backend!r} skin must be WGSL source text")
    source = None if skin is None else skin.strip()
    if source == "":
        raise ValueError(f"provider {feature!r}/{backend!r} skin is empty")

    text = str(label if label is not None else backend).strip()
    records = _PROVIDERS.get(feature)
    if records is None:
        records = _PROVIDERS.setdefault(feature, {})
    before = _selected(feature)
    existing = records.get(backend)
    order = existing["order"] if existing is not None else _PROVIDER_SERIAL
    if existing is None:
        _PROVIDER_SERIAL += 1
    records[backend] = {
        "feature": feature,
        "backend": backend,
        "label": text or backend,
        "command": command,
        "priority": int(priority),
        "order": order,
        "activate": activate,
        "deactivate": deactivate,
        "refresh": refresh,
        "settings": schema,
        "skin": source,
    }
    _PROVIDER_REPORTED.difference_update(
        {key for key in _PROVIDER_REPORTED if key[0] == feature and key[1] == backend}
    )
    _SETTING_REPORTED.difference_update(
        {key for key in _SETTING_REPORTED if key[0] == feature and key[1] == backend}
    )
    for spec in schema:
        if spec["kind"] not in _SETTING_KINDS:
            _report_setting(
                feature,
                backend,
                spec["name"],
                ValueError(
                    f"unknown kind {spec['kind']!r}; the row is read-only"
                ),
            )

    # A takeover is a selection change caused by registration. Stop the outgoing
    # backend, but leave the incoming one for the first explicit switch (which
    # keeps ``activate`` out of import time).
    selected = _selected(feature)
    if selected == backend and before is not None and before != backend:
        outgoing = records.get(before)
        if outgoing is not None:
            _deactivate_on_takeover(outgoing, feature)
            if _PROVIDER_APPLIED.get(feature) == before:
                _PROVIDER_APPLIED.pop(feature, None)
    return _manifest(records[backend], feature)


def refreshProviders(feature: str) -> dict:
    """Ask the active backend of ``feature`` to re-read the shared state.

    This is the counterpart of ``activate``/``deactivate`` for a change that is
    not a switch: a command (or a dialog) already applied a request, and a
    backend that draws from its own store has to look at the shared store again
    — the grid toggle writes ``FerroCAD.Grid.enabled`` and the Coin overlay then
    re-binds from it. Like the other provider callbacks it runs **only when
    asked**, never at registration or on a switch, and a failure is reported and
    collected instead of raised.

    Only the active backend runs; an absent ``refresh`` callback (or no provider
    for the feature) is a no-op. A callback may return the same dict a command
    may return, so ``view`` carries a request the caller can merge (the grid
    toggle reads the ``grid_overlay`` claim from it). Returns ``ok``,
    ``feature``, ``backend``, ``view``, ``status`` and ``errors``; it never
    raises for a backend problem.
    """
    result = {
        "ok": True,
        "feature": feature,
        "backend": None,
        "view": {},
        "status": "",
        "errors": [],
    }
    records = _PROVIDERS.get(feature)
    if not records:
        return result
    backend = _selected(feature)
    result["backend"] = backend
    statuses = []
    _invoke_provider(records[backend], feature, "refresh", result, statuses)
    if statuses:
        result["status"] = "; ".join(statuses)
    return result


def _deactivate_on_takeover(record, feature) -> None:
    """Stop a backend whose feature changed hands while a mod was registering."""
    callback = record.get("deactivate")
    if callback is None:
        return
    try:
        callback()
    except Exception as exc:  # noqa: BLE001 - a broken backend is not fatal
        _report_provider(feature, record["backend"], "deactivate", exc)


def unregisterProvider(feature: str, backend: str) -> bool:
    """Drop a provider record; returns whether it existed.

    This is a reload/teardown helper and does **not** run ``deactivate``; the
    caller owns the effects (the Coin mod, for example, shuts its manager down
    before re-registering). A persisted choice is deliberately left alone, so a
    re-registered backend keeps its selection.
    """
    records = _PROVIDERS.get(feature)
    if not records or backend not in records:
        return False
    del records[backend]
    if not records:
        _PROVIDERS.pop(feature, None)
    if _PROVIDER_APPLIED.get(feature) == backend:
        _PROVIDER_APPLIED.pop(feature, None)
    _PROVIDER_REPORTED.difference_update(
        {key for key in _PROVIDER_REPORTED if key[0] == feature and key[1] == backend}
    )
    _SETTING_REPORTED.difference_update(
        {key for key in _SETTING_REPORTED if key[0] == feature and key[1] == backend}
    )
    return True


def _manifest(record, feature, active=None):
    """One provider as JSON-safe data (the callbacks never leave this module)."""
    if active is None:
        active = _selected(feature)
    return {
        "feature": feature,
        "backend": record["backend"],
        "label": record["label"],
        "command": record["command"],
        "priority": record["priority"],
        "active": record["backend"] == active,
    }


def _selected(feature):
    """The backend that *is* the provider: the persisted choice, else priority.

    A persisted value that no longer names a registered backend is ignored (a
    mod was removed or renamed); the highest priority wins. An equal priority is
    broken by registration order, earliest first, so the answer is deterministic.
    """
    records = _PROVIDERS.get(feature)
    if not records:
        return None
    saved = _provider_prefs().GetString(feature, "")
    if saved in records:
        return saved
    return max(
        records.values(),
        key=lambda record: (record["priority"], -record["order"]),
    )["backend"]


def providers(feature=None) -> list:
    """The provider manifests, in registration order.

    With a ``feature``, only that feature's backends (``[]`` when none are
    registered); without one, every feature in first-registration order. Each
    entry carries ``feature``, ``backend``, ``label``, ``command``, ``priority``
    and ``active``. The callbacks stay private so the list stays JSON-safe.
    """
    if feature is not None:
        records = _PROVIDERS.get(feature)
        if not records:
            return []
        active = _selected(feature)
        return [_manifest(record, feature, active) for record in records.values()]
    entries = []
    for name in _PROVIDERS:
        active = _selected(name)
        entries.extend(
            _manifest(record, name, active) for record in _PROVIDERS[name].values()
        )
    return entries


def activeProvider(feature):
    """The active backend for ``feature``, or ``None`` when none is registered.

    This is the declaration (the persisted choice, else the highest priority).
    A mod checks it in ``IsActive``/``Activated`` to refuse to run when another
    backend owns the feature.
    """
    return _selected(feature)


def providerSkin(feature, backend=None):
    """The WGSL material program of ``feature``'s backend, or ``None``.

    This is the host's channel for a provider-supplied shader skin: the shell
    asks a capability that hosts one (the manipulator gizmo) for its material
    program and draws with the source :func:`registerProvider` recorded under
    ``skin``. ``backend`` names a specific backend; ``None`` — the form the
    shell uses — means the **active** backend. That is the point: a switch makes
    the host ask again, so a stale skin cannot survive a provider change.

    ``None`` also means "draw the native material": an unknown feature or
    backend, and a backend that supplied no skin, all answer ``None``. Like
    :func:`activeProvider` this never raises for an unknown name, because the
    shell polls it across the interpreter boundary.
    """
    records = _PROVIDERS.get(feature)
    if not records:
        return None
    if backend is None:
        backend = _selected(feature)
    record = records.get(backend)
    if record is None:
        return None
    return record.get("skin")


def setActiveProvider(feature: str, backend: str) -> dict:
    """Make ``backend`` the active provider for ``feature`` and apply it.

    The choice is persisted first, then the outgoing backend is deactivated and
    the incoming one activated, each at most once per change. The outgoing
    backend is the applied one when there is one, else the declared active one,
    so a first switch still restores the default it left. Selecting the
    already-declared backend runs its ``activate`` once (the bootstrap case).

    Callback errors are caught and reported (never fatal), as command callbacks
    are; they are listed under ``errors``. Returns a dict with ``ok``,
    ``feature``, ``backend``, ``previous``, the merged ``view`` request and a
    one-line ``status``. An unknown feature or backend raises ``ValueError``.
    """
    global _PROVIDER_DOCUMENT

    records = _PROVIDERS.get(feature)
    if not records:
        raise ValueError(f"no provider is registered for feature {feature!r}")
    if backend not in records:
        raise ValueError(f"{backend!r} does not provide feature {feature!r}")

    # The declared active backend before the choice changes. It is the outgoing
    # one even when its effects were never applied: its ``deactivate`` is an
    # idempotent "restore the default", so the first switch still leaves the
    # viewport consistent. Selecting the already-declared default is the one case
    # that still runs ``activate`` once, which is how a backend is bootstrapped.
    declared = _selected(feature)
    applied = _PROVIDER_APPLIED.get(feature)
    outgoing_backend = applied if applied is not None else declared
    _provider_prefs().SetString(feature, backend)

    result = {
        "ok": True,
        "feature": feature,
        "backend": backend,
        "previous": declared,
        "view": {},
        "status": "",
        "errors": [],
    }
    statuses = []
    if outgoing_backend != backend:
        outgoing = records.get(outgoing_backend) if outgoing_backend is not None else None
        if outgoing is not None:
            _invoke_provider(outgoing, feature, "deactivate", result, statuses)
        _invoke_provider(records[backend], feature, "activate", result, statuses)
    elif applied is None:
        _invoke_provider(records[backend], feature, "activate", result, statuses)
    _PROVIDER_APPLIED[feature] = backend
    document = FreeCAD.activeDocument()
    if document is not None:
        # Explicit switches are a document attachment too; recording it here
        # keeps the close/deactivate path correct even when a caller drove only
        # the registry (a script, or a document created through upstream
        # ``FreeCAD`` rather than the FerroCAD lifecycle wrappers).
        _PROVIDER_DOCUMENT = document.Name

    message = f"{feature}: {backend} is the active provider"
    if statuses:
        message = "; ".join(statuses) + "; " + message
    if result["errors"]:
        message += " (reported: " + "; ".join(result["errors"]) + ")"
    result["status"] = message
    return result


def _invoke_provider(record, feature, hook, result, statuses) -> None:
    """Run one provider callback, isolating and collecting its result."""
    callback = record.get(hook)
    if callback is None:
        return
    try:
        value = callback()
    except Exception as exc:  # noqa: BLE001 - one broken backend is not fatal
        _report_provider(feature, record["backend"], hook, exc)
        result["errors"].append(f"{hook}: {_error_text(exc)}")
        return
    if not isinstance(value, dict):
        return
    view = value.get("view")
    if isinstance(view, dict):
        result["view"].update(view)
    status = value.get("status")
    if status:
        statuses.append(str(status))


# ---------------------------------------------------------------------------
# Provider attachment follows the active document
#
# The registry applies a backend's effects once (activate) and undoes them on a
# switch (deactivate). Those effects may be per-document — the Coin grid builds
# one overlay per document — so the document lifecycle must drive the same
# callbacks. A closed document's attachment used to survive until the next
# activation; this observer makes deactivation immediate.
# ---------------------------------------------------------------------------


class _ProviderDocumentObserver:
    """Attach the applied providers to the active document and detach them."""

    def onDocumentActivated(self, name) -> None:
        _activate_document_providers(name)

    def onDocumentDeactivated(self, name) -> None:
        _deactivate_document_providers(name)

    def onDocumentClosed(self, name) -> None:
        _deactivate_document_providers(name)


def _activate_document_providers(name) -> None:
    """Run every provider's ``activate`` for the document becoming active.

    This is the bootstrap path as well as the re-attach after a switch: the
    declared backend of every registered feature becomes drawn again. It is
    idempotent per document, so a caller that reports both a creation and an
    activation (or an activation twice) does not run ``activate`` twice.
    """
    global _PROVIDER_DOCUMENT
    if not isinstance(name, str) or not name:
        return
    if _PROVIDER_DOCUMENT == name:
        return
    if _PROVIDER_DOCUMENT is not None:
        # A caller skipped the deactivate event (upstream ``FreeCAD`` created or
        # activated a document directly); stop the stale attachment first.
        _deactivate_document_providers(_PROVIDER_DOCUMENT)
    for feature, records in list(_PROVIDERS.items()):
        backend = _selected(feature)
        record = records.get(backend)
        if record is None:
            continue
        result = {
            "ok": True,
            "feature": feature,
            "backend": backend,
            "view": {},
            "status": "",
            "errors": [],
        }
        _invoke_provider(record, feature, "activate", result, [])
        _PROVIDER_APPLIED[feature] = backend
    _PROVIDER_DOCUMENT = name


def _deactivate_document_providers(name) -> None:
    """Run the applied providers' ``deactivate`` for the document leaving.

    The applied record is dropped **before** the callback runs, so a document
    that is deactivated and then closed (or closed twice) cannot call
    ``deactivate`` twice. Once a provider is no longer applied, the next
    activation bootstraps it again through :func:`_activate_document_providers`.
    """
    global _PROVIDER_DOCUMENT
    if _PROVIDER_DOCUMENT != name:
        return
    _PROVIDER_DOCUMENT = None
    for feature, backend in list(_PROVIDER_APPLIED.items()):
        _PROVIDER_APPLIED.pop(feature, None)
        records = _PROVIDERS.get(feature)
        record = records.get(backend) if records else None
        if record is None:
            continue
        result = {
            "ok": True,
            "feature": feature,
            "backend": backend,
            "view": {},
            "status": "",
            "errors": [],
        }
        _invoke_provider(record, feature, "deactivate", result, [])


addDocumentListener(_ProviderDocumentObserver())


# ---------------------------------------------------------------------------
# Provider settings (S0, docs/project/record/tasks/settings-model.md)
#
# A provider declares its knobs once, at registration, and FerroCAD owns the
# store: one parameter group per ``(feature, backend)``, one typed key per
# setting. The rows are the property pane's row contract, so the tree and the
# pane render a provider's settings without a second renderer. Nothing is
# written at registration: a default is materialised on read, which keeps
# importing a mod side-effect free.
# ---------------------------------------------------------------------------


def _settings_group(feature, backend):
    """The parameter group holding one backend's setting values."""
    return FreeCAD.ParamGet(f"{PROVIDER_PREFERENCES}/{feature}/{backend}")


def _validate_settings(feature, backend, settings) -> list:
    """Validate a provider's schema and return its normalised form.

    A malformed schema is a programmer error at registration time, so it raises
    ``TypeError``/``ValueError`` with the offending entry named. An unknown kind
    is deliberately **not** an error: it is allowed, reported once by the
    caller, and rendered read-only.
    """
    if settings is None:
        return []
    if not isinstance(settings, (list, tuple)):
        raise TypeError(
            f"provider {feature!r}/{backend!r} settings must be a list of dicts"
        )
    schema = []
    names = set()
    for index, entry in enumerate(settings):
        if not isinstance(entry, dict):
            raise TypeError(
                f"provider {feature!r}/{backend!r} setting #{index} is not a dict"
            )
        name = entry.get("name")
        if not isinstance(name, str) or not name.strip():
            raise ValueError(
                f"provider {feature!r}/{backend!r} setting #{index} needs a "
                "non-empty 'name'"
            )
        name = name.strip()
        if name in names:
            raise ValueError(
                f"provider {feature!r}/{backend!r} declares setting {name!r} twice"
            )
        names.add(name)
        kind = entry.get("kind")
        if not isinstance(kind, str) or not kind.strip():
            raise ValueError(
                f"provider {feature!r}/{backend!r} setting {name!r} needs a 'kind'"
            )
        spec = {
            "name": name,
            "kind": kind.strip(),
            "label": str(entry.get("label") or name),
        }
        unit = entry.get("unit")
        if unit is not None:
            if not isinstance(unit, str) or not unit.strip():
                raise ValueError(
                    f"provider {feature!r}/{backend!r} setting {name!r} has a "
                    "non-string 'unit'"
                )
            spec["unit"] = unit.strip()
        choices = entry.get("choices")
        if choices is not None:
            if not isinstance(choices, (list, tuple)) or not all(
                isinstance(choice, str) and choice for choice in choices
            ):
                raise ValueError(
                    f"provider {feature!r}/{backend!r} setting {name!r} needs "
                    "'choices' to be a list of non-empty strings"
                )
            spec["choices"] = list(choices)
        if spec["kind"] == "enum" and not spec.get("choices"):
            raise ValueError(
                f"provider {feature!r}/{backend!r} setting {name!r} has kind "
                "'enum' but no 'choices'"
            )
        for bound in ("min", "max"):
            value = entry.get(bound)
            if value is None:
                continue
            if isinstance(value, bool) or not isinstance(value, (int, float)):
                raise TypeError(
                    f"provider {feature!r}/{backend!r} setting {name!r} "
                    f"'{bound}' must be a number"
                )
            spec[bound] = float(value)
        if "min" in spec and "max" in spec and spec["min"] > spec["max"]:
            raise ValueError(
                f"provider {feature!r}/{backend!r} setting {name!r} has "
                f"min > max ({spec['min']:g} > {spec['max']:g})"
            )
        for hook in ("get", "set"):
            callback = entry.get(hook)
            if callback is None:
                continue
            if not callable(callback):
                raise TypeError(
                    f"provider {feature!r}/{backend!r} setting {name!r} "
                    f"'{hook}' is not callable"
                )
            spec[hook] = callback
        if "default" in entry:
            default = entry["default"]
            if spec["kind"] in _SETTING_KINDS:
                default = _coerce_setting(spec, default)
            spec["default"] = default
        elif spec["kind"] == "enum":
            spec["default"] = spec["choices"][0]
        else:
            spec["default"] = _SETTING_DEFAULTS.get(spec["kind"], "")
        schema.append(spec)
    return schema


def _coerce_setting(spec, value):
    """Coerce one value to the setting's kind, enforcing range and choices.

    Raises ``ValueError`` (with the setting named) when the value does not fit;
    callers turn that into an error result and leave the store untouched.
    """
    kind = spec["kind"]
    name = spec["name"]
    if kind == "text":
        result = value if isinstance(value, str) else str(value)
    elif kind == "bool":
        if isinstance(value, bool):
            result = value
        elif isinstance(value, (int, float)):
            result = bool(value)
        elif isinstance(value, str):
            text = value.strip().lower()
            if text in ("1", "true", "yes", "on"):
                result = True
            elif text in ("0", "false", "no", "off", ""):
                result = False
            else:
                raise ValueError(f"setting {name!r} expects true or false")
        else:
            raise ValueError(f"setting {name!r} expects true or false")
    elif kind == "int":
        if isinstance(value, bool):
            raise ValueError(f"setting {name!r} expects an integer")
        try:
            number = float(value) if isinstance(value, str) else value
        except (TypeError, ValueError):
            raise ValueError(f"setting {name!r} expects an integer") from None
        if isinstance(number, float) and not number.is_integer():
            raise ValueError(f"setting {name!r} expects an integer")
        try:
            result = int(number)
        except (TypeError, ValueError):
            raise ValueError(f"setting {name!r} expects an integer") from None
    elif kind == "float":
        if isinstance(value, bool):
            raise ValueError(f"setting {name!r} expects a number")
        try:
            result = float(value)
        except (TypeError, ValueError):
            raise ValueError(f"setting {name!r} expects a number") from None
    elif kind == "quantity":
        result = _quantity_value(spec, value)
    elif kind == "enum":
        text = value if isinstance(value, str) else str(value)
        if text not in spec["choices"]:
            choices = ", ".join(repr(choice) for choice in spec["choices"])
            raise ValueError(
                f"setting {name!r} must be one of {choices}; got {text!r}"
            )
        result = text
    elif kind == "color":
        text = str(value).strip()
        if not _COLOR_RE.match(text):
            raise ValueError(
                f"setting {name!r} expects a colour as #rrggbb; got {text!r}"
            )
        result = text.lower()
    else:
        raise ValueError(f"setting {name!r} has unsupported kind {kind!r}")

    if kind in ("int", "float", "quantity"):
        minimum = spec.get("min")
        maximum = spec.get("max")
        if minimum is not None and result < minimum:
            raise ValueError(
                f"setting {name!r} must be at least {minimum:g}; got {result:g}"
            )
        if maximum is not None and result > maximum:
            raise ValueError(
                f"setting {name!r} must be at most {maximum:g}; got {result:g}"
            )
    return result


def _quantity_value(spec, value) -> float:
    """A quantity as a number in the setting's ``unit`` (or its own value)."""
    name = spec["name"]
    unit = spec.get("unit")
    if isinstance(value, FreeCAD.Units.Quantity):
        quantity = value
    elif isinstance(value, bool):
        raise ValueError(f"setting {name!r} expects a quantity")
    elif isinstance(value, (int, float)):
        # A bare number means "that many of the declared unit".
        quantity = (
            FreeCAD.Units.Quantity(float(value), unit)
            if unit
            else FreeCAD.Units.Quantity(float(value))
        )
    else:
        text = str(value).strip()
        try:
            number = float(text)
        except ValueError:
            number = None
        if number is not None:
            # A bare numeric string is "that many of the declared unit", like
            # a bare Python number.
            quantity = (
                FreeCAD.Units.Quantity(number, unit)
                if unit
                else FreeCAD.Units.Quantity(number)
            )
        else:
            try:
                quantity = FreeCAD.Units.Quantity(text)
            except Exception as exc:
                raise ValueError(
                    f"setting {name!r} expects a quantity: {exc}"
                ) from None
    if not unit:
        return quantity.Value
    try:
        return quantity.getValueAs(unit).Value
    except Exception as exc:
        raise ValueError(
            f"setting {name!r} expects a quantity in {unit!r}: {exc}"
        ) from None


def _display_setting(spec, value) -> str:
    """The row's display string (``"10 mm"`` for a quantity)."""
    if spec["kind"] != "quantity":
        return str(value)
    if isinstance(value, FreeCAD.Units.Quantity):
        return str(value)
    unit = spec.get("unit")
    if not unit:
        return str(value)
    try:
        return str(FreeCAD.Units.Quantity(float(value), unit))
    except Exception:
        return str(value)


def _read_setting(spec, group):
    """``(value, status)`` for one schema entry, without writing anything.

    ``status`` names the source of the value: ``computed`` for a ``get`` hook,
    ``provider`` for the parameter group (a stored value or the materialised
    default), ``unsupported kind`` for a kind the editor cannot edit.
    """
    getter = spec.get("get")
    if getter is not None:
        return getter(), "computed"
    kind = spec["kind"]
    name = spec["name"]
    default = spec["default"]
    if kind not in _SETTING_KINDS:
        return default, "unsupported kind"
    if kind == "bool":
        return group.GetBool(name, bool(default)), "provider"
    if kind == "int":
        return group.GetInt(name, int(default)), "provider"
    if kind in ("float", "quantity"):
        return group.GetFloat(name, float(default)), "provider"
    return group.GetString(name, str(default)), "provider"


def _write_setting(spec, group, value) -> None:
    """Store a coerced value in the provider's parameter group (by kind)."""
    kind = spec["kind"]
    name = spec["name"]
    if kind == "bool":
        group.SetBool(name, bool(value))
    elif kind == "int":
        group.SetInt(name, int(value))
    elif kind in ("float", "quantity"):
        group.SetFloat(name, float(value))
    else:
        group.SetString(name, str(value))


def settings(feature: str, backend=None) -> list:
    """The settings rows of ``feature``'s active provider (or ``backend``).

    The rows use the property pane's contract — ``name``, ``label``, ``type``,
    ``value``, ``status``, ``editable``, ``kind``, plus ``unit``/``min``/``max``
    and ``choices`` where the schema declares them — so the pane needs no new
    renderer. ``value`` is the display string (``"10 mm"`` for a quantity);
    ``min``/``max`` are what make a bounded row offer the range slider.

    Nothing is written here: an unset value shows its declared default. ``[]``
    means "nothing to show": an unknown feature or backend, no provider at all,
    or a provider that advertises no schema.
    """
    records = _PROVIDERS.get(feature)
    if not records:
        return []
    if backend is None:
        backend = _selected(feature)
    record = records.get(backend)
    if record is None:
        return []
    schema = record.get("settings") or []
    if not schema:
        return []
    group = _settings_group(feature, backend)
    rows = []
    for spec in schema:
        try:
            raw, status = _read_setting(spec, group)
        except Exception as exc:  # noqa: BLE001 - one broken knob is not fatal
            _report_setting(feature, backend, spec["name"], exc)
            raw, status = spec["default"], "error"
        kind = spec["kind"]
        has_get = spec.get("get") is not None
        has_set = spec.get("set") is not None
        row = {
            "name": spec["name"],
            "label": spec["label"],
            "type": _SETTING_TYPES.get(kind, ""),
            "value": _display_setting(spec, raw),
            "status": status,
            "editable": kind in _SETTING_KINDS and (has_set or not has_get),
            "kind": kind,
        }
        if "unit" in spec:
            row["unit"] = spec["unit"]
        if "min" in spec:
            row["min"] = spec["min"]
        if "max" in spec:
            row["max"] = spec["max"]
        if "choices" in spec:
            row["choices"] = list(spec["choices"])
        rows.append(row)
    return rows


def setSetting(feature: str, name: str, value) -> dict:
    """Coerce, validate and store one setting, then refresh the provider.

    The value may be a string from the property editor (``"5 mm"``,
    ``"true"``, ``"#94a3b8"``) or a Python value of the right type. On success
    the parameter is written (or the schema's ``set`` hook is called), the
    active backend's ``refresh`` runs through :func:`refreshProviders`, and the
    result carries its ``view`` claim so the caller can apply it. On a
    validation failure nothing changes and the result has ``ok: False`` and an
    ``error``.
    """
    records = _PROVIDERS.get(feature)
    if not records:
        return {
            "ok": False,
            "error": f"no provider is registered for feature {feature!r}",
        }
    backend = _selected(feature)
    schema = records[backend].get("settings") or []
    spec = next((entry for entry in schema if entry["name"] == name), None)
    if spec is None:
        return {
            "ok": False,
            "error": f"no setting {name!r} for feature {feature!r}",
        }
    kind = spec["kind"]
    if kind not in _SETTING_KINDS:
        return {
            "ok": False,
            "error": f"setting {name!r} has unsupported kind {kind!r}",
        }
    setter = spec.get("set")
    if spec.get("get") is not None and setter is None:
        return {"ok": False, "error": f"setting {name!r} is read-only"}
    try:
        coerced = _coerce_setting(spec, value)
    except Exception as exc:  # noqa: BLE001 - validation is an expected failure
        return {"ok": False, "error": str(exc)}

    if setter is not None:
        try:
            setter(coerced)
        except Exception as exc:  # noqa: BLE001 - a broken hook is not fatal
            _report_setting(feature, backend, name, exc)
            return {"ok": False, "error": _error_text(exc)}
    else:
        try:
            _write_setting(spec, _settings_group(feature, backend), coerced)
        except Exception as exc:  # noqa: BLE001
            return {"ok": False, "error": _error_text(exc)}

    refresh = refreshProviders(feature)
    display = _display_setting(spec, coerced)
    return {
        "ok": True,
        "feature": feature,
        "backend": backend,
        "name": name,
        "value": display,
        "status": f"{feature}: {name} = {display}",
        "view": refresh.get("view", {}),
        "errors": list(refresh.get("errors", [])),
    }


# ---------------------------------------------------------------------------
# Edition appearance presets
#
# An edition (toy, parametric, a future one) ships an opinion about what its
# gizmo, axis cross and grid should look like. That opinion belongs *under* the
# user's settings, not beside them: the persisted parameter group is the user
# layer, and the edition only supplies values where the user has not chosen one.
#
# The semantic facades (``FerroCAD.Gizmo`` / ``Axis`` / ``Grid``) read the
# provider group directly, so the preset is materialised into that same group
# (no parallel store) and a marker remembers which values the preset wrote. On a
# later install a value is replaced only when it still equals the marker, so a
# user override survives both a restart and a preset change. Resetting is the
# inverse: clear the capability store, then install the preset again.
# ---------------------------------------------------------------------------

#: The provider feature -> the private semantic module that owns its schema.
_APPEARANCE_MODULES = {"gizmo": "_gizmo", "axis": "_axis", "grid": "_grid"}

#: The provider feature -> the public facade attribute (``FerroCAD.Gizmo`` ...).
_APPEARANCE_FACADES = {"gizmo": "Gizmo", "axis": "Axis", "grid": "Grid"}

#: The appearance features every edition may preset, in reset order.
_APPEARANCE_FEATURES = ("gizmo", "axis", "grid")

#: The backend whose parameter group is the semantic store. The facades keep
#: their values in the native provider's group even when another backend draws.
_APPEARANCE_NATIVE = "ferrocad"

#: The key that records the values the last install wrote (a JSON object). The
#: value is provenance, not a setting: ``settings`` and the facades ignore it.
_APPEARANCE_PRESET_MARKER = "__appearance_preset__"

#: ``feature -> {setting: value}`` declared by the edition, in declaration order.
_APPEARANCE_PRESETS: dict = {}


def _semantic_module(feature):
    """The private semantic module that owns ``feature``'s schema, or ``None``.

    Upstream FreeCAD (and a headless build without the capability) has no
    ``FerroCAD._gizmo``; a preset is then kept but cannot be installed.
    """
    module_name = _APPEARANCE_MODULES.get(feature)
    if module_name is None:
        return None
    try:
        return importlib.import_module("FerroCAD." + module_name)
    except Exception:  # noqa: BLE001 - no semantic API is a normal upstream build
        return None


def _appearance_facade(feature):
    """``FerroCAD.Gizmo``/``Axis``/``Grid`` for ``feature``, or ``None``."""
    attribute = _APPEARANCE_FACADES.get(feature)
    if attribute is None:
        return None
    try:
        import FerroCAD
    except Exception:  # noqa: BLE001 - upstream FreeCAD has no FerroCAD
        return None
    try:
        return getattr(FerroCAD, attribute)
    except Exception:  # noqa: BLE001 - the capability is missing
        return None


def _semantic_specs(feature):
    """The capability's canonical ``name -> schema entry`` map."""
    module = _semantic_module(feature)
    if module is None:
        return {}
    return {entry["name"]: entry for entry in module.SETTINGS_SCHEMA}


def _preset_group(feature):
    """The parameter group that holds ``feature``'s resolved values."""
    return _settings_group(feature, _APPEARANCE_NATIVE)


def _preset_marker(group):
    """The provenance record of the last install, ``{}`` when absent/corrupt."""
    try:
        raw = group.GetString(_APPEARANCE_PRESET_MARKER, "")
        data = json.loads(raw) if raw else {}
    except Exception:  # noqa: BLE001 - a corrupt marker must not block boot
        return {}
    return data if isinstance(data, dict) else {}


def _stored_value(group, name):
    """The value in ``group`` under ``name``, or ``None`` when it is unset.

    The typed lists distinguish an unset value from a stored default; a typed
    ``Get*`` call cannot.
    """
    if name in group.GetBools():
        return group.GetBool(name)
    if name in group.GetInts():
        return group.GetInt(name)
    if name in group.GetFloats():
        return group.GetFloat(name)
    if name in group.GetStrings():
        return group.GetString(name)
    return None


def _remove_value(group, name):
    """Drop ``name`` from every scalar kind in ``group`` (absent is a no-op)."""
    group.RemBool(name)
    group.RemInt(name)
    group.RemFloat(name)
    group.RemString(name)


def _write_preset_value(group, name, value, spec) -> None:
    """Write one preset value under the schema's kind (or the value's own type)."""
    if spec is not None:
        _write_setting(spec, group, value)
        return
    if isinstance(value, bool):
        group.SetBool(name, value)
    elif isinstance(value, int):
        group.SetInt(name, value)
    elif isinstance(value, float):
        group.SetFloat(name, value)
    else:
        group.SetString(name, str(value))


def _spec_for(feature, name):
    """The schema entry for ``name``: the capability's, else the provider's.

    The preset belongs to the capability store, so the capability's canonical
    kind wins even while another backend is active. A provider schema is only a
    fallback for a feature without a semantic module (a third-party capability).
    """
    spec = _semantic_specs(feature).get(name)
    if spec is not None:
        return spec
    records = _PROVIDERS.get(feature)
    if records:
        active = _selected(feature)
        order = [active] + [backend for backend in records if backend != active]
        for backend in order:
            record = records.get(backend)
            if record is None:
                continue
            for spec in record.get("settings") or []:
                if spec["name"] == name:
                    return spec
    return None


def _normalise_preset(feature, values):
    """Validate a declaration against the capability schema (or its value types)."""
    if not isinstance(feature, str) or not feature.strip():
        raise ValueError("an appearance feature must be a non-empty string")
    feature = feature.strip()
    if not isinstance(values, dict):
        raise TypeError(
            "appearance preset %r must be a dict of setting -> value" % feature
        )
    specs = _semantic_specs(feature)
    preset = {}
    for name, value in values.items():
        if not isinstance(name, str) or not name.strip():
            raise ValueError("an appearance setting name must be a non-empty string")
        name = name.strip()
        spec = specs.get(name)
        if specs and spec is None:
            known = ", ".join(sorted(specs))
            raise ValueError(
                "appearance preset %r has no setting %r; known: %s"
                % (feature, name, known)
            )
        if spec is not None:
            preset[name] = _coerce_setting(spec, value)
        elif isinstance(value, (bool, int, float, str)):
            preset[name] = value
        else:
            raise TypeError(
                "appearance setting %r of %r must be a bool, number or string"
                % (name, feature)
            )
    return feature, preset


def registerAppearancePreset(feature: str, values: dict) -> dict:
    """Declare an edition's default appearance for ``feature`` (declaration only).

    ``feature`` is ``"gizmo"``, ``"axis"`` or ``"grid"`` for the built-in
    capabilities; ``values`` maps a setting name (the schema's ``name``) to a
    value. A name the capability does not declare, or a value its schema rejects,
    raises immediately, so a typo fails at import rather than silently doing
    nothing.

    Declaring stores the preset and returns a JSON-safe manifest; it does **not**
    write a parameter. The edition's ``bootstrap()`` calls
    :func:`installAppearancePresets` (and the shell can call
    :func:`appearanceView`), which materialises the preset only where the user
    has no value. Re-declaring replaces the previous preset; call
    :func:`unregisterAppearancePreset` to remove it.
    """
    feature, preset = _normalise_preset(feature, values)
    _APPEARANCE_PRESETS[feature] = preset
    return {"feature": feature, "settings": dict(preset)}


def unregisterAppearancePreset(feature: str) -> bool:
    """Remove a declared preset and any values it still owns.

    Values the user changed are left alone. Returns whether a preset was
    declared. This is the reload/teardown counterpart of
    :func:`registerAppearancePreset`.
    """
    if feature not in _APPEARANCE_PRESETS:
        return False
    _install_preset(feature, {})
    del _APPEARANCE_PRESETS[feature]
    return True


def appearancePreset(feature: str):
    """A copy of the declared preset for ``feature``, or ``None`` when none is."""
    preset = _APPEARANCE_PRESETS.get(feature)
    return None if preset is None else dict(preset)


def appearancePresets() -> dict:
    """Every declared preset, ``{feature: {setting: value}}``, in declaration order."""
    return {feature: dict(preset) for feature, preset in _APPEARANCE_PRESETS.items()}


def _install_preset(feature, preset) -> int:
    """Materialise ``preset`` without overwriting a user value; return the writes.

    The marker holds the values the previous install wrote. A value is "owned"
    when it still equals that record, or when it is absent on a first install.
    Only owned values are written or removed; anything else is a user override.

    The record cannot distinguish a user value that happens to equal the preset
    from the preset's own write: a later preset change may replace that one
    value. Every other user value is preserved.
    """
    group = _preset_group(feature)
    previous = _preset_marker(group)
    writes = 0
    for name in sorted(set(previous) | set(preset)):
        stored = _stored_value(group, name)
        if name in previous:
            owned = type(stored) is type(previous[name]) and stored == previous[name]
        else:
            # First install: fill only an unset value. A value already present
            # was chosen by the user (or another preset) and stays.
            owned = stored is None
        if not owned:
            continue
        if name in preset:
            spec = _spec_for(feature, name)
            value = preset[name]
            if spec is not None:
                value = _coerce_setting(spec, value)
            before = stored
            _write_preset_value(group, name, value, spec)
            if before is None or type(before) is not type(value) or before != value:
                writes += 1
        else:
            _remove_value(group, name)
            writes += 1
    if preset:
        group.SetString(
            _APPEARANCE_PRESET_MARKER, json.dumps(preset, sort_keys=True)
        )
    elif _APPEARANCE_PRESET_MARKER in group.GetStrings():
        group.RemString(_APPEARANCE_PRESET_MARKER)
    return writes


def installAppearancePresets(feature=None) -> dict:
    """Materialise declared presets under the user's values (idempotent).

    With no ``feature``, every declared preset is installed; with one, only that
    feature's. Nothing is written where the user already has a value, and a
    value the previous preset wrote is replaced only when it is unchanged. This
    is the boot step; ``FerroCADGui.resetAppearance`` calls it after clearing the
    store. Returns ``ok``, the features that were installed, the write count per
    feature and any errors (a broken setting does not stop the others).
    """
    if feature is None:
        features = list(_APPEARANCE_PRESETS)
    elif isinstance(feature, str) and feature.strip():
        features = [feature.strip()]
    else:
        raise ValueError("an appearance feature must be a non-empty string or None")
    writes = {}
    errors = []
    for name in features:
        preset = _APPEARANCE_PRESETS.get(name)
        if preset is None:
            continue
        try:
            writes[name] = _install_preset(name, preset)
        except Exception as exc:  # noqa: BLE001 - one feature is not fatal
            errors.append("%s: %s" % (name, _error_text(exc)))
    return {
        "ok": not errors,
        "features": list(writes),
        "writes": writes,
        "errors": errors,
    }


def _appearance_request_features(feature):
    """The features a view request covers: one, or the three capabilities."""
    if feature is None:
        return list(_APPEARANCE_FEATURES)
    if not isinstance(feature, str) or not feature.strip():
        raise ValueError("an appearance feature must be a non-empty string or None")
    return [feature.strip()]


def _feature_view(feature):
    """One capability's current ``{"view": …}`` request, or ``None``."""
    facade = _appearance_facade(feature)
    if facade is None:
        return None
    return facade.apply()


def appearanceView(feature=None) -> dict:
    """The ``{"view": …}`` request for the current appearance policy.

    This is the boot hook: it installs the declared presets, then asks each
    capability (``FerroCAD.Gizmo``/``Axis``/``Grid``) for its current policy, so
    an edition's defaults and a user's persisted values both reach the shell.
    With no ``feature`` the three built-in capabilities are merged; with one,
    only that feature is. Errors are collected rather than raised, because the
    shell may call this across the interpreter boundary.
    """
    installAppearancePresets(feature)
    view = {}
    errors = []
    for name in _appearance_request_features(feature):
        try:
            request = _feature_view(name)
        except Exception as exc:  # noqa: BLE001 - one capability is not fatal
            errors.append("%s: %s" % (name, _error_text(exc)))
            continue
        if isinstance(request, dict):
            view.update(request.get("view") or {})
    return {"view": view, "errors": errors}


def _clear_axis_legacy() -> None:
    """Drop the pre-split ``GridAxisCross`` flags that outlive an axis reset.

    ``FerroCAD.Axis`` migrates that old key when ``enabled`` is unset. A
    deliberate appearance reset must remove it, or the migrated user value would
    veto the edition preset it is supposed to restore.
    """
    module = _semantic_module("axis")
    paths = (
        getattr(
            module,
            "LEGACY_GRID_PREFERENCES",
            "User parameter:BaseApp/Preferences/Mods/grid/ferrocad",
        ),
        getattr(
            module,
            "LEGACY_PREFERENCES",
            "User parameter:BaseApp/Preferences/View",
        ),
    )
    for path in paths:
        FreeCAD.ParamGet(path).RemBool("GridAxisCross")


def resetAppearance(feature=None) -> dict:
    """Restore the edition preset (or the built-in defaults) for ``feature``.

    The capability store is cleared first (including the grid's legacy keys and
    the axis's old ``GridAxisCross``), then the declared preset is installed
    again, so the reset returns the edition's values rather than the hard-coded
    defaults. The result carries the merged ``view`` request a command returns;
    errors are collected, never raised. With no ``feature`` the three built-in
    capabilities are reset together.
    """
    features = _appearance_request_features(feature)
    view = {}
    statuses = []
    errors = []
    for name in features:
        if name not in _APPEARANCE_FEATURES and name not in _APPEARANCE_PRESETS:
            errors.append("no appearance settings for %r" % name)
            continue
        try:
            facade = _appearance_facade(name)
            if facade is not None:
                facade.reset()
            else:
                _preset_group(name).Clear()
            if name == "axis":
                _clear_axis_legacy()
            preset = _APPEARANCE_PRESETS.get(name, {})
            _install_preset(name, preset)
            if facade is not None:
                request = facade.apply()
                view.update(request.get("view") or {})
            statuses.append(
                "%s reset to the edition preset" % name
                if preset
                else "%s reset to defaults" % name
            )
        except Exception as exc:  # noqa: BLE001 - one feature is not fatal
            errors.append("%s: %s" % (name, _error_text(exc)))
    return {
        "ok": not errors,
        "feature": feature,
        "features": features,
        "view": view,
        "status": "; ".join(statuses),
        "errors": errors,
    }


# ---------------------------------------------------------------------------
# Pull-based overlays (G5)
# ---------------------------------------------------------------------------


def _coin_module():
    """The ``pivy.coin`` module, imported only when an overlay is used.

    Headless builds and the GUI facade itself do not need Coin, so this import is
    deliberately lazy: ``import FerroCADGui`` keeps working without the extension.
    """
    try:
        from pivy import coin
    except ImportError as exc:  # noqa: BLE001 - re-raised with the contract
        raise RuntimeError(
            "overlays need the Coin facade (pivy.coin) in this build"
        ) from exc
    return coin


def addOverlay(node) -> None:
    """Register a Coin node as a viewport overlay (FerroCAD extension).

    The node is **extracted now**: the shell keeps a copy of its subtree and draws
    it beside the document scene on the next refresh. This is the pull bridge that
    replaces Coin's ``getSceneGraph()``; see ``docs/developer/workbenches/grid-overlay-plan.md`` §3.2.
    A later in-place edit does not reach the copy, so an author re-registers the
    node (``removeOverlay`` then ``addOverlay``) after changing it.
    """
    return _coin_module().addOverlay(node)


def removeOverlay(node) -> bool:
    """Drop a previously registered overlay, keyed by Python object identity."""
    return _coin_module().removeOverlay(node)


# ---------------------------------------------------------------------------
# View state (read-only; pushed by the shell)
# ---------------------------------------------------------------------------


class _ViewState:
    """The shell's presentation state, pushed by the shell (read-only here).

    ``grid`` is the reference-grid flag, ``axis_cross`` the world triad's, ``projection``
    is ``"persp"`` or ``"ortho"``, ``grid_minor``/``grid_major`` are the world grid's
    steps (mm) and ``grid_radius`` is a fixed extent (mm, or ``None`` for the
    camera-derived one). A checkable command reads these in ``IsChecked``, which is what
    makes a ``{"view": …}`` request close the loop.
    """

    grid = False
    axis_cross = True
    projection = "persp"
    grid_minor = 10.0
    grid_major = 100.0
    grid_radius = None


ViewState = _ViewState()


__all__ = [
    "addDocumentListener",
    "removeDocumentListener",
    "registerProvider",
    "unregisterProvider",
    "providers",
    "providerSkin",
    "activeProvider",
    "setActiveProvider",
    "refreshProviders",
    "settings",
    "setSetting",
    "registerAppearancePreset",
    "unregisterAppearancePreset",
    "appearancePreset",
    "appearancePresets",
    "installAppearancePresets",
    "appearanceView",
    "resetAppearance",
    "addOverlay",
    "removeOverlay",
    "ViewState",
    "PROVIDER_PREFERENCES",
]
