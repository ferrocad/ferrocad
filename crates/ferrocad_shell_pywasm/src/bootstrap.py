"""The host gaps a `wasm32-unknown-unknown` RustPython cannot fill itself.

Everything above this file is the desktop's own source: `python/FreeCAD`,
`python/FreeCADGui`, `python/FerroCADGui` and the `python/FerroCAD` package are run from
`python/` (a native host imports the tree from disk, see `lib.rs`), and `ferrocad` is the
native module in `native.rs`. This file fills the gaps a `wasm32-unknown-unknown`
RustPython has: the target has no POSIX module, so RustPython's frozen `os`/`_io` layer is
off and six stdlib modules cannot be imported:

- ``contextlib`` — ``suppress`` and ``redirect_stdout``/``redirect_stderr``
- ``io`` — ``StringIO``, the console's stdout capture
- ``pathlib`` — ``Path(name).with_name(...)``, the shell's catalogue read
- ``traceback`` — ``format_exc``, which the frozen build answers with ``""``
- ``importlib`` — ``import_module`` (the shell) and ``util.find_spec`` (``FreeCAD``'s
  built-in probe); the frozen ``importlib`` cannot load without ``posix``
- ``os`` — the handful of path and environment names the shared modules touch

Two more shims belong to the same group: ``tempfile`` (the parameter store's atomic save,
which this host never runs because it has no parameter file) and the ``_io`` file layer
behind the console's redirects.

None of it is policy: a missing module would stop the shared source from importing, and
supplying the names it touches is what lets one facade run on both interpreters.
"""

import sys


def _module(name, namespace):
    """A module named ``name`` whose attributes are ``namespace``."""
    module = type(sys)(name)
    module.__dict__.update(namespace)
    module.__dict__["__name__"] = name
    sys.modules[name] = module
    return module


def _missing(name, *attributes):
    """Whether ``name`` is absent, or lacks one of the ``attributes`` we need.

    With ``host_env`` on (every native build) the real stdlib modules are here, and
    the shared facade should use them — the parameter store reads and writes a real
    file through ``os``, ``io``, ``tempfile`` and the builtin ``open``. The wasm target
    has no POSIX module, so those imports fail and the substitutes below cover the
    handful of names the facade touches. A partially linked module is replaced too.
    """
    try:
        module = __import__(name)
    except Exception:  # noqa: BLE001 - a missing or broken module is the same answer
        return True
    return any(not hasattr(module, attribute) for attribute in attributes)


# --- the modules wasm32 cannot import -------------------------------------


class _StringIO:
    """Text buffer, enough for ``ferrocad_shell.evaluate``'s stdout capture."""

    def __init__(self, initial=""):
        self._chunks = [initial]

    def write(self, text):
        self._chunks.append(str(text))
        return len(str(text))

    def writelines(self, lines):
        for line in lines:
            self.write(line)

    def getvalue(self):
        return "".join(self._chunks)

    def flush(self):
        pass

    def close(self):
        pass

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        return False


class _Suppress:
    def __init__(self, *exceptions):
        self._exceptions = exceptions or (Exception,)

    def __enter__(self):
        return self

    def __exit__(self, kind, value, tb):
        return kind is not None and issubclass(kind, self._exceptions)


class _Redirect:
    """``contextlib.redirect_stdout``/``redirect_stderr`` over ``sys``."""

    def __init__(self, attribute, target):
        self._attribute = attribute
        self._target = target

    def __enter__(self):
        self._saved = getattr(sys, self._attribute, None)
        setattr(sys, self._attribute, self._target)
        return self._target

    def __exit__(self, *exc):
        setattr(sys, self._attribute, self._saved)
        return False


class _Path:
    """The slice of ``pathlib`` the shell uses (``Path(...).name``/``with_name``)."""

    def __init__(self, *parts):
        self._text = "/".join(str(part) for part in parts)

    def __str__(self):
        return self._text

    @property
    def name(self):
        return self._text.rsplit("/", 1)[-1]

    @property
    def parent(self):
        head, _, _ = self._text.rpartition("/")
        return _Path(head or ".")

    @property
    def suffix(self):
        name = self.name
        return "" if "." not in name else "." + name.rsplit(".", 1)[-1]

    def with_name(self, name):
        """``Path(__file__).with_name(...)``, the shell's import-time catalogue read."""
        head, _, _ = self._text.rpartition("/")
        return _Path(head + "/" + name if head else name)

    def is_file(self):
        return False

    def exists(self):
        return False

    def __truediv__(self, other):
        return _Path(self._text, other)

    def __eq__(self, other):
        return isinstance(other, _Path) and str(self) == str(other)

    def __hash__(self):
        return hash(self._text)


def _import_module(name):
    """`importlib.import_module` over the frozen importer.

    The frozen ``importlib`` cannot load in a ``host_env``-off build:
    ``importlib._bootstrap_external`` imports ``posix``, which the wasm target does not
    have. The shared modules call ``import_module`` and ``util.find_spec``, and
    ``__import__`` is the same operation.
    """
    return __import__(name, fromlist=["*"])


def _find_spec(name):
    """`importlib.util.find_spec`: no module has a spec here.

    ``python/FreeCAD`` uses it to tell a built-in extension from one on ``sys.path``
    (`_builtin`). Nothing is on a path in this build, and the modules it asks about are
    linked or absent, so ``None`` is the honest answer.
    """
    return None


class _BuiltinImporter:
    """The marker ``python/FreeCAD`` compares a spec's loader against."""


if _missing("io", "StringIO"):
    _module("io", {"StringIO": _StringIO, "TextIOBase": object, "BytesIO": _StringIO})
if _missing("contextlib", "suppress"):
    _module(
        "contextlib",
        {
            "suppress": _Suppress,
            "redirect_stdout": lambda target: _Redirect("stdout", target),
            "redirect_stderr": lambda target: _Redirect("stderr", target),
        },
    )
if _missing("pathlib", "Path"):
    _module("pathlib", {"Path": _Path, "PurePath": _Path})
if _missing("traceback", "format_exc"):
    _module("traceback", {"format_exc": lambda: ""})
if _missing("importlib", "import_module", "util", "machinery"):
    _module(
        "importlib",
        {
            "import_module": _import_module,
            "util": _module("importlib.util", {"find_spec": _find_spec}),
            "machinery": _module(
                "importlib.machinery", {"BuiltinImporter": _BuiltinImporter}
            ),
        },
    )


class _OsPathLike:
    """`os.PathLike`: the marker class `FreeCADGui.open_overlay` checks."""


def _os_fspath(value):
    """`os.fspath`: a path string, or the object's `__fspath__` result."""
    if isinstance(value, str):
        return value
    try:
        return value.__fspath__()
    except AttributeError:
        raise TypeError("expected str or os.PathLike, got %r" % (value,)) from None


def _os_fsdecode(value):
    """`os.fsdecode`: the filesystem path as text (there is one encoding here)."""
    value = _os_fspath(value)
    return value.decode("utf-8") if isinstance(value, bytes) else value


class _TextReader:
    """A tiny text file over one fetched mod resource."""

    def __init__(self, text, name=""):
        self._text = text
        self._offset = 0
        self.name = name

    def read(self, size=-1):
        if size is None or size < 0:
            chunk = self._text[self._offset:]
            self._offset = len(self._text)
            return chunk
        chunk = self._text[self._offset:self._offset + size]
        self._offset += len(chunk)
        return chunk

    def readline(self, size=-1):
        newline = self._text.find("\n", self._offset)
        end = len(self._text) if newline < 0 else newline + 1
        return self.read(end - self._offset)

    def readlines(self):
        lines = []
        while True:
            line = self.readline()
            if not line:
                return lines
            lines.append(line)

    def __iter__(self):
        return iter(self.readlines())

    def close(self):
        pass

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.close()
        return False


try:  # the interpreter's own `open`; absent on wasm, where there is no filesystem
    _real_open = open
except NameError:  # pragma: no cover - the wasm build
    _real_open = None


def _pack_key(path):
    """The installed resource `path` names, or ``None``.

    The loader names a mod's entry ``<name>/<script>``, and the host installs resources
    under that same ``<name>/`` root. A native host's ``os.path.abspath`` then prepends
    the process directory, so the absolute form is accepted too: the table is rooted
    where the loader put it, not where the desktop's current directory happens to be.
    """
    try:
        import ferrocad as _native

        if _native.file_exists(path):
            return path
        import os

        prefix = os.getcwd().rstrip("/") + "/"
        if path.startswith(prefix):
            relative = path[len(prefix):]
            if _native.file_exists(relative):
                return relative
    except Exception:  # noqa: BLE001 - no table, no cwd: nothing to serve
        pass
    return None


def _pack_open(file, mode="r", *args, **kwargs):
    """`open` that serves a fetched mod's ``Resources/`` first.

    A pack carries its resources beside its entry scripts, and the host installs each at
    the path the mod computes from ``__file__`` (``<name>/Resources/...``). Anything not
    in that table is the interpreter's own file, which is what a desktop host with
    ``host_env`` has; on wasm there is none, so an absent path is ``FileNotFoundError``
    exactly as it was before.
    """
    path = file.decode("utf-8") if isinstance(file, bytes) else str(file)
    if not any(flag in mode for flag in "wax+"):
        key = _pack_key(path)
        if key is not None:
            if "b" in mode:
                raise OSError("pywasm: mod resources are text; binary reads are not available")
            import ferrocad as _native

            return _TextReader(_native.read_text(key), path)
    if _real_open is None:
        raise FileNotFoundError(path)
    return _real_open(file, mode, *args, **kwargs)


def _pack_exists(path):
    """Whether a fetched mod resource is installed (``os.path.exists``/``isfile``)."""
    return _pack_key(str(path)) is not None


# One `open` for both hosts: a pack's resources first, the interpreter's files after.
import builtins as _builtins

_builtins.open = _pack_open


# `os` is only importable with `host_env`, which the wasm build does not have (a native
# host gets the real module and this block is skipped). The shared modules use these
# names for path-like arguments and for the parameter store's environment lookup; there
# is no filesystem behind any of them (`open` itself refuses, see the module docs), so a
# path question answers what it can and a read does not exist.
if _missing("os", "environ", "fdopen", "fspath"):
    _module(
        "os",
        {
            "PathLike": _OsPathLike,
        "fspath": _os_fspath,
        "fsdecode": _os_fsdecode,
        "sep": "/",
        "altsep": None,
        "linesep": "\n",
        "environ": {},
            "path": _module(
                "os.path",
                {
                    "join": lambda *parts: "/".join(str(part) for part in parts),
                    "dirname": lambda path: str(path).rpartition("/")[0],
                    "basename": lambda path: str(path).rpartition("/")[2],
                    "abspath": lambda path: str(path),
                    "expanduser": lambda path: str(path),
                    "exists": _pack_exists,
                    "isfile": _pack_exists,
                    "isdir": lambda path: False,
                },
            ),
        },
    )


def _no_tempfile(*args, **kwargs):
    """`tempfile.mkstemp`: the wasm host has no temporary files.

    The parameter store is memory-only here (the loader clears its file path), so the
    atomic save never runs; an explicit save says why instead of failing obscurely.
    """
    raise OSError("pywasm: this host has no temporary files")


if _missing("tempfile", "mkstemp"):
    _module("tempfile", {"mkstemp": _no_tempfile})


def configure(freecad, store, path):
    """Wire the installed App facade to this host.

    ``store`` is ``"memory"`` or ``"file"``; ``path`` is the parameter file the host
    resolved (empty for a memory store). A memory store clears the facade's path, so the
    values last for the process; a file store points the facade at ``path`` and swaps in
    this host's two file operations, because the interpreter has no ordinary filesystem
    layer here. Either way the host has a window, so ``GuiUp`` is set — that is what the
    Gui branches (the tree card, the view mirror) read.
    """
    freecad.GuiUp = 1
    if store == "memory" or not path:
        freecad._parameter_file_path = lambda: None
        return
    native = __import__("ferrocad")
    freecad._configure_parameter_file(path)
    freecad._read_parameter_file = native.read_parameter_file
    freecad._write_parameter_file = native.write_parameter_file


# The loader (Rust) calls `configure` from its own scope; a module is how it gets there.
_module("_ferrocad_bootstrap", {"configure": configure})
