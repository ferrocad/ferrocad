"""Python-declarative UI spike.

A standalone, hermetic version of the eventual ``FreeCAD/ui/declarative.py``.
A workbench builds an ``Element`` tree in Python; ``render_ui()`` serializes the
full tree (initial state), and ``dispatch_event(handler_id)`` runs a callback
then emits a *diffed patch stream* against the previous tree, so only changes
cross the PyO3 boundary.
"""

import json
from typing import Callable, Dict, List, Optional


class Element:
    def __init__(self, tag: str, **attributes):
        self.tag: str = tag
        self.attributes: Dict = dict(attributes)
        self.children: List["Element"] = []
        self.event_handlers: Dict[str, Callable] = {}
        self.id: str = attributes.get("id", "")

    def on(self, event_name: str, handler: Callable) -> "Element":
        self.event_handlers[event_name] = handler
        return self

    def add(self, child: "Element") -> "Element":
        self.children.append(child)
        return self

    def serialize(self, registry: Dict[str, Callable]) -> dict:
        event_keys = []
        for evt, handler in self.event_handlers.items():
            handler_id = f"{self.id}_{evt}"
            registry[handler_id] = handler
            event_keys.append([evt, handler_id])

        return {
            "id": self.id,
            "tag": self.tag,
            "props": {k: v for k, v in self.attributes.items() if k != "id"},
            "events": event_keys,
            "children": [c.serialize(registry) for c in self.children],
        }


class AppState:
    def __init__(self):
        self.counter: int = 0
        self.label_prefix: str = "Total Operations: "

    def increment(self):
        self.counter += 1

    def fail_gracefully(self):
        raise RuntimeError("Controlled workbench fault")


app_state = AppState()
ACTIVE_REGISTRY: Dict[str, Callable] = {}
_prev_tree: Optional[dict] = None


def reset_state() -> None:
    """Reset module-global state (test isolation between Rust tests)."""
    global _prev_tree
    app_state.counter = 0
    _prev_tree = None


def _snapshot() -> dict:
    ACTIVE_REGISTRY.clear()

    root = Element("container", id="root", layout="vertical")

    label = Element(
        "label",
        id="counter_label",
        text=f"{app_state.label_prefix}{app_state.counter}",
    )

    btn_increment = Element(
        "button", id="btn_inc", title="Increment Count"
    ).on("click", lambda: app_state.increment())

    btn_fault = Element(
        "button", id="btn_err", title="Trigger Error"
    ).on("click", lambda: app_state.fail_gracefully())

    root.add(label).add(btn_increment).add(btn_fault)

    return root.serialize(ACTIVE_REGISTRY)


def render_ui() -> str:
    """Return the full tree (used only for the initial render)."""
    global _prev_tree
    _prev_tree = _snapshot()
    return json.dumps(_prev_tree)


def _diff_nodes(old, new, patches):
    if old is None:
        patches.append({"op": "insert", "node": new})
        return
    if new is None:
        patches.append({"op": "remove", "id": old.get("id")})
        return

    changed = {}
    for key in ("text", "title"):
        ov = old.get("props", {}).get(key)
        nv = new.get("props", {}).get(key)
        if ov != nv:
            changed[key] = nv
    if changed:
        patches.append({"op": "update", "id": new["id"], "props": changed})

    old_children = {c["id"]: c for c in old.get("children", [])}
    new_children = {c["id"]: c for c in new.get("children", [])}
    for cid in old_children:
        if cid not in new_children:
            patches.append({"op": "remove", "id": cid})
    for cid, nc in new_children.items():
        _diff_nodes(old_children.get(cid), nc, patches)


def _diff(old, new):
    patches = []
    _diff_nodes(old, new, patches)
    return patches


def dispatch_event(handler_id: str) -> str:
    """Run a handler, then return the diffed patch stream (JSON array)."""
    global _prev_tree
    if handler_id in ACTIVE_REGISTRY:
        # Executes the callback; may raise to test boundary isolation.
        ACTIVE_REGISTRY[handler_id]()
    new_tree = _snapshot()
    patches = _diff(_prev_tree, new_tree)
    _prev_tree = new_tree
    return json.dumps(patches)
