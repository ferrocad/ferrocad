# Architecture: layers, call paths, and the C-ABI question

Status: reference (2026-10-05). Companion to
[`python-bindings.md`](python-bindings.md), [`milestones.md`](milestones.md) and
[`releasing.md`](releasing.md).

This note answers three questions that the crate list alone does not make
obvious:

1. What calls what, at runtime?
2. Does `ferrocad_gpui` call `ferrocad_core` directly, through PyO3, or both?
3. Is the `ctypes` backend (`ferrocad_ctypes`) needed, or is it dead weight?

## 1. The components

| Component | Kind | Role |
| --- | --- | --- |
| `ferrocad_types` | `rlib` | Base value types (FreeCAD's `Base`): `Quantity`/`Unit`, `Vector3`/`Matrix4`/`Rotation`/`Placement`. A **leaf**. |
| `ferrocad_geom` | `rlib` | The geometry seam: opaque `Shape`, `History`/`ElementRef`/`ElementMap`, and the `GeometryBackend` trait (+ `NullBackend`). No kernel. A **leaf** above `ferrocad_types`. |
| `ferrocad_occt` | `rlib` | The OCCT implementation of `GeometryBackend` (box/fuse/cut/fillet/place), via `opencascade-sys` + the sibling history bridge. Needs OCCT to build; **not** a default member. |
| `ferrocad_core` | `rlib` | The pure-Rust engine: documents, properties, DAG, transactions, observers, expressions. No Python, no UI, no OCCT. Depends on `ferrocad_types` (and re-exports it). A **leaf** wrt the kernel. |
| `ferrocad_py` | `cdylib` | The PyO3 extension, importable as the module `ferrocad`. The only bridge into `ferrocad_core`. |
| `ferrocad_widgets` | `rlib` | Reusable `bite-gpui` widgets. Depends on **neither** `ferrocad_core` nor `ferrocad_py`. |
| `ferrocad_gpui` | `rlib` | The `bite-gpui` application-shell library: boots CPython and provides `Shell`, `run`/`run_with` and `HostConfig`. Ships no application scripts. |
| `ferrocad` | `bin` | The base app (the general edition). Owns the app's Python entry module and, when packaged, its mods and interpreter; calls `ferrocad_gpui::run_with`. |
| `python/FreeCAD` | package | The public, pure-Python `import FreeCAD` facade. |
| `ferrocad_gen` | `cdylib` | Generated PyO3 skeletons (M3c). A build-time experiment, not wired into the facade. |
| `ferrocad_ctypes` | `cdylib` | The legacy M0 C-ABI backend, **removed** (kept in §4 as the rationale). |

## 2. Who owns the process: there are two entry points

The stack starts in exactly one of two ways.

**A. Rust owns the process** (the app shell):

```
ferrocad_gpui
   |  embeds CPython (pyo3 "auto-initialize")
   v
CPython interpreter
   |  import ferrocad_shell ; import FreeCAD
   v
FreeCAD facade (python/FreeCAD)          <- pure Python
   |  import ferrocad  (a built-in module when the app links it)
   v
ferrocad_py  (module `ferrocad`)         <- PyO3
   |
   v
ferrocad_core                            <- pure Rust
```

**B. Python owns the process** (scripts, tests, upstream conformance):

```
python3 (hello_freecad.py, tests/, tools/conformance.py)
   |  import FreeCAD
   v
FreeCAD facade (python/FreeCAD)
   |
   v
ferrocad_py  (module `ferrocad`)
   |
   v
ferrocad_core
```

The facade and everything below it are identical in both. The only difference is
whether `ferrocad_gpui` or `python3` started the interpreter.

## 3. "Does the host call core, PyO3, and CPython?"

The host's `[dependencies]` are `ferrocad_widgets`, `bite-gpui`, `pyo3`, `serde`,
`serde_json`. It names **neither `ferrocad_core` nor `ferrocad_py`**. So of the
three candidate paths:

| Path | Built? | Notes |
| --- | --- | --- |
| `host -> ferrocad_core` (direct Rust call) | **no** | A real, legitimate option. A GIL-free Rust path is exactly what a viewport render loop wants later. It is simply not needed yet, because the shell reads the model through the facade. |
| `host -> ferrocad_py -> ferrocad_core` (as a Rust library) | **no, and not possible** | `ferrocad_py` is a `cdylib` whose entire API is `#[pyclass]`/`#[pyfunction]`. It has no plain Rust entry point; it is reachable only through an interpreter. This is not a separate path. |
| `host -> CPython -> FreeCAD -> ferrocad_py -> ferrocad_core` | **yes** | This is the one path that runs, and it is path A of §2. |

So the sketch of "three paths" collapses to **one live path**, plus a
possible-but-unbuilt direct Rust path. "host -> pyo3 -> core" is not distinct
from "host -> CPython -> ... -> core": the `pyo3` crate is what does the
embedding, and the module it reaches is the same `ferrocad_py`.

## 4. The `ctypes` backend is not a second bridge. It is a second model.

This is the source of the confusion. A C-ABI backend would normally mean "the
same core, reached through a flat ABI":

```
CLAIMED:  FreeCAD -> ctypes -> ferrocad_ctypes -> ferrocad_core
```

But `ferrocad_ctypes` does **not** depend on `ferrocad_core`. It defines its own
`FcDocument`, `FcObject` and `Property`, its own global registry, and its own
naming rules. What actually exists is:

```
REALITY:  FreeCAD -> ctypes -> ferrocad_ctypes (its own, smaller model)
                               ^ never touches ferrocad_core
```

So `FreeCAD.backend` does not choose a *transport*. It chooses between two
independent implementations. That is why it is confusing: the fallback is not
slower PyO3, it is a different, narrower, and separately-maintained document
model that happens to share a name.

## 5. Is the C ABI needed? No.

The three reasons given in `python-bindings.md` §4 do not hold:

| Claimed reason | Reality |
| --- | --- |
| "Builds with no Python headers." | A build-environment constraint, not a deployment requirement. FreeCAD **is** a Python application: a CPython is always present, and building any extension needs its headers. CI installs `python3-dev`; the sandbox has them. |
| "CPython-implementation-agnostic escape hatch if `abi3` fails." | `abi3` already decouples the wheel from the CPython version. The C ABI only helps a **non-CPython** interpreter (PyPy, GraalPy), which FerroCAD does not target. |
| "Proves the core is not PyO3-shaped." | False as built: `ferrocad_ctypes` does not call `ferrocad_core` at all, so it proves nothing about the core. |

The capability check settles it: PyO3 can express everything the C ABI can
(functions, opaque handles, strings), plus real classes, signatures and
exceptions that the C ABI has to rebuild by hand. There is **no operation
reachable only through the C ABI**. Therefore the C ABI is not "absolutely
needed", and the correct move is a hard dependency on the CPython headers, with
the fallback deleted. **That removal has been applied** (2026-10-05):

* [x] deleted `crates/ferrocad_ctypes/`, its `members` entry and `Cargo.lock` record;
* [x] deleted `python/FreeCAD/_ctypes_backend.py` and `python/FreeCAD/_ffi.py`;
* [x] simplified `python/FreeCAD/__init__.py` to import `ferrocad` unconditionally
  (`backend = "ferrocad"` is still reported, which the shell surfaces);
* [x] dropped the ctypes copy from `build.sh` and the ctypes step from
  `.github/workflows/ci.yml`;
* [x] updated `README.md`, `python-bindings.md`, `mvp-path.md`, `rewrite-strategy.md`,
  `milestones.md`, and the book's Chapter 14 and Chapter 16.

The one genuine future use of a C ABI would be a **non-Python host** (a C++ or
game-engine embedder). If that ever happens, the ABI must be built *over*
`ferrocad_core` (so there is still one model). The deleted crate would not have
served that purpose even if it had been kept.

## 6. Dependency rules

These edges are the ones to keep. `ferrocad_core` is a leaf; nothing ferrocad
depends on a `cdylib`.

```
ferrocad_types     -> (crates.io only)               [leaf: Quantity, Placement, ...]
ferrocad_geom      -> ferrocad_types                 [seam: Shape, History, GeometryBackend]
ferrocad_occt      -> ferrocad_geom, ferrocad_types,
                      opencascade-sys                [kernel backend; never -> core]
ferrocad_core      -> ferrocad_types                 [re-exports; no OCCT]
ferrocad_py        -> ferrocad_core                  [rlib + cdylib]
ferrocad_widgets   -> (bite-gpui only)               [must not depend on core or py]
ferrocad_gpui      -> ferrocad_widgets, pyo3 (embed) [may later -> ferrocad_core directly]
                      must NOT -> ferrocad_py        [the *app* links it, not the shell]
ferrocad           -> ferrocad_gpui, ferrocad_py     [base app: links the module in, embeds payload]
python/FreeCAD     -> ferrocad (built-in module when linked, else a .so on sys.path)
```

`ferrocad_geom` is not yet depended on and `ferrocad_occt` is not yet installed:
`ferrocad_core` gains the `ferrocad_geom` edge (and an `App` backend slot) when a
`Property` can hold a `Shape`, at which point the application installs `OcctBackend`.
The backend sits above the seam and never below `ferrocad_core`; see
[`occt-integration.md`](occt-integration.md).

The rule that keeps the design honest: **the shell talks to the model through the
`FreeCAD` API, exactly like a workbench does.** A direct `shell -> ferrocad_core`
edge is a permitted future optimisation for the render loop, but it must be
added deliberately, not by accident, and it must never bypass the same
`ferrocad_core` that Python uses.

## 7. Editions and repackaging

The shell library is the base of a family of product editions (FerroCAD: Architecture,
Furniture, and so on). The plan is a shell **library** plus thin per-edition
binaries, not forks; that is why the crate is `ferrocad_gpui` and not named for a
platform. See [`repackaging.md`](repackaging.md).

## 8. Where the Python lives

The shell library (`ferrocad_gpui`) owns the **interpreter**: it boots CPython,
puts directories on `sys.path`, and marshals JSON. It ships **no** application
scripts, so it can serve any app or edition.

The application (`ferrocad`, or an edition) owns the **scripts**: the entry module
it names in `HostConfig.entry_module`, and the directories in
`HostConfig.python_paths`. In the repository these are `python/ferrocad_shell` and
the `FreeCAD` facade (development); in a packaged build they are the app's startup
scripts and its `Mod/` workbenches, shipped inside the distribution beside the
bundled interpreter.

This is FreeCAD's own split: the interpreter belongs to the application, while the
workbench (`Mod/`) scripts ship with the distribution.

The sources stay in one canonical tree (`python/` at the repository root).
Packaging copies them (maturin for the wheel, `xtask` for the app bundle) rather
than duplicating them into a crate; see [`distribution.md`](distribution.md) §8.

Workbench scripts (`mods/*/Init.py`, `InitGui.py`) are loaded at boot
**best-effort**: a script that raises is reported with its traceback and the app
continues. Nothing about a broken workbench is fatal.
