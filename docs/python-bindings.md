# Python bindings: PyO3 vs the C-API (working note)

Status: reference (2026-10-04). Companion to
[`rewrite-strategy.md`](rewrite-strategy.md), [`milestones.md`](milestones.md)
(M0, M1, M3b, M3c) and [`app-shell-vision.md`](app-shell-vision.md). It explains
how Rust and Python are bridged in FerroCAD, the trade-offs between the available
mechanisms, and how that choice reaches the packager.

## 1. The job

The product surface is `import FreeCAD`, and the engine is Rust. The two have to
talk in two directions:

- **Extension module.** Python owns the process; it imports a native module and
  calls into Rust. This is the `ferrocad` wheel.
- **Embedding.** Rust owns the process; it starts a CPython interpreter and runs
  Python (workbench scripts, the console). This is `ferrocad_gpui`.

Both need a bridge, and the bridge can be built at three levels: the CPython
C-API directly, a Rust binding to that C-API (PyO3), or a plain C ABI that Python
loads with `ctypes`/`cffi`.

## 2. The mechanisms

**Raw CPython C-API.** You write `extern "C"` functions, a `PyMethodDef` table, a
`PyModuleDef`, and `PyTypeObject` structs, and you return `PyObject*`. Reference
counts are manual (`Py_INCREF`/`Py_DECREF`), failures return `NULL` with an
exception set, and every type is a hand-filled struct. It has no dependency beyond
`Python.h`, gives exact control, and works on any CPython. It is also the most
error-prone option: a missed decref is a leak, an extra one is a use-after-free,
and both are silent until they are not.

**PyO3.** Rust bindings to that C-API. You write `#[pyclass]` structs and
`#[pymethods]` methods; arguments and returns are typed (`i64`, `String`,
`Vec<f64>`), errors are `PyResult<T>`, and the GIL is a token (`Python::with_gil`)
rather than a call you can forget. It manages refcounts through `Bound`/`Py`
smart pointers, converts Rust errors to Python exceptions via `From`, and ships an
`abi3` mode and the `extension-module` feature that make correct wheels easy. The
cost is a dependency with its own `0.x` API churn (`with_gil` replaced `acquire_gil`,
`Bound` replaced `Py`), a compile-time cost, and an abstraction that can hide the
cost of per-call conversions.

**C ABI + ctypes.** Rust exposes a flat `extern "C"` surface over opaque handles
(`create_document() -> *mut Doc`, `doc_set_property(...) -> i32`). Python loads the
shared library with `ctypes` and wraps it in ordinary Python classes. No
`Python.h`, no interpreter at build time, no CPython ABI dependency at all: the
`.so` is a normal Rust `cdylib` and the object model lives in Python. The costs are
real: you marshal every value by hand (numbers, strings, and any structured data
become JSON or a bespoke ABI), calls are slower, there is no type checking across
the boundary, and the Python side has to reconstruct the classes the C-API would
have given you for free.

**cffi.** The same idea as `ctypes` with a C header (or ABI description) and a
compiler at build time. Closer to a real FFI, still a C ABI. Same trade.

**Others.** `rust-cpython` predates PyO3; `PyOxidizer` packages an interpreter
into a binary (an embedding/packaging tool, not a binding style); `uniffi`/`napi`
target other languages. None change the three-level shape.

## 3. Trade-offs

| Dimension | Raw C-API | PyO3 | C ABI + ctypes |
| --- | --- | --- | --- |
| Build needs `Python.h` | yes | yes (unless `abi3`) | no |
| ABI coupling | per CPython version (or `abi3`) | per version (or `abi3`) | none |
| Memory safety of the glue | manual refcounts | managed | n/a (opaque handles) |
| Ergonomics | low | high | low, and split across two languages |
| Type fidelity | real Python objects | real Python objects | handles; Python class written by hand |
| Per-call cost | 1 FFI call | 1 FFI call + conversions | 1 FFI call + marshalling, higher |
| Error handling | return `NULL`, set exception | `PyResult` / `PyErr` | status code + message string |
| GIL | manual | a token you hold | n/a (no interpreter at build) |
| Tooling / wheels | manual `setuptools` | `maturin` | ship the `.so` as package data |
| Main risk | refcount bugs | version churn, hidden costs | marshalling bugs, no type checking |

## 4. What FerroCAD chose, and why

**PyO3 is the primary backend** (`crates/ferrocad_py`): `Document` and
`DocumentObject` are `#[pyclass]` types over `Arc<Mutex<ferrocad_core::Document>>`,
built with `abi3` and `extension-module` (M1, M3b). It gave the real `FreeCAD`
object model with the least glue, and `maturin` turns it into a wheel.

**A C-ABI backend was kept as a fallback** (`crates/ferrocad_ctypes`, M0): a flat
`extern "C"` surface loaded by `python/FreeCAD/_ctypes_backend.py`. It was
introduced when the build sandbox had no CPython headers. It was **not** a thin
bridge over `ferrocad_core`: it re-implemented its own smaller document model, so
it was a second implementation, not a second transport. The three original
justifications (no headers needed, an `abi3` escape hatch, "proves the core is
not PyO3-shaped") did not survive review, and it **has been removed** (2026-10-05).
See [`architecture.md`](architecture.md) §4-5 for the call-path diagrams and the
verdict.

The facade (`python/FreeCAD/__init__.py`) now imports the extension
unconditionally and reports `FreeCAD.backend == "ferrocad"`.

The generated skeleton (`crates/ferrocad_gen`, M3c) is a third path: code
generated from the `.pyi` surface. It is a build-time experiment, not a shipped
backend.

## 5. Extension module vs embedding

The two directions need opposite build flags, and mixing them up is a common
failure:

- **Extension module** (`ferrocad_py`): PyO3 must *not* link `libpython`; the
  interpreter that imports the module already provides it. Hence the opt-in
  `extension-module` feature. Linking a second `libpython` produces a wheel that
  imports and then crashes.
- **Embedding** (`ferrocad_gpui`): Rust initializes CPython
  (`pyo3::prepare_freethreaded_python` / `Python::with_gil`) and imports the
  `FreeCAD` facade. Here PyO3 *must* link `libpython`. The host does not link the
  extension crate; it loads the facade, which imports the `ferrocad` module the
  ordinary way.

Because the same crate cannot be both at once, the feature is opt-in and the two
binaries are built separately. This is the concrete reason the workspace keeps
`extension-module` off by default.

## 6. abi3

`abi3` builds against CPython's stable ABI: one wheel works on every CPython from
the floor upward, so the wheel matrix is platform times architecture, not platform
times interpreter. The costs are a reduced C-API surface (only the stable subset)
and a floor to keep current (`requires-python`, currently 3.10). This sandbox runs
CPython 3.14, newer than PyO3 0.25 targets, so the build sets
`PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1`; that is an escape hatch, not something to
ship a policy around.

## 7. Errors, panics and the GIL

- **Errors.** PyO3: return `PyResult<T>`; `?` propagates a Rust error once it
  implements `From<...> for PyErr` (the pattern used for `Property::set` failures
  mapped to `PyValueError`). C-ABI: return a status and read a message back.
- **Panics.** A Rust panic that unwinds across the FFI boundary is undefined
  behavior. PyO3 catches panics at the boundary and converts them to exceptions;
  hand-written `extern "C"` code must catch them (`catch_unwind`) itself.
- **GIL.** PyO3's `with_gil` is a token; do not hold it while rendering. The shell
  takes it only around interpreter calls (and serializes tests with `PYTHON_LOCK`).
  For long Rust work inside a call, `allow_threads` releases it.

## 8. How this feeds packaging

The choice of bridge decides what ships:

- PyO3 + `abi3` + `extension-module` becomes a `maturin` wheel (one per platform).
- The C-ABI fallback is a `.so` shipped as package data inside the `FreeCAD`
  package.
- The embedding host is a desktop application that must ship (or depend on) an
  interpreter; that is the packaging problem of the app, not of the wheel.

The book's packaging chapter covers the wheel side; this note is the reason its
`pyproject.toml` lines read as they do.

## 9. Open questions

- How much of the `FreeCAD` surface the C-ABI fallback should cover before it is
  the same product, versus a rescue path.
- The `abi3` floor over time, and whether any per-version feature is worth giving
  it up.
- Whether the generated backend (`ferrocad_gen`) graduates from experiment to the
  way the surface is maintained.
