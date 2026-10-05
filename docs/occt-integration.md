# OCCT integration: crate layout and the backend seam

Status: decision note (2026-10-06), companion to
[`geometry-and-topology.md`](geometry-and-topology.md) §6 (the `GeometryEngine` seam),
[`occt-history-spike.md`](occt-history-spike.md) (the verified binding) and
[`occt-bundling.md`](occt-bundling.md) (shipping the kernel).

The rule from [`architecture.md`](architecture.md) §6 stands: **`ferrocad_core` is a
leaf and OCCT must never be a transitive dependency of it.** This note turns that rule
into a crate layout, and settles where a Part workbench and its Python bindings sit.

## 1. Answering the two questions

**"Does `ferrocad_part` depend on `ferrocad_core`?"** Yes. A Part feature *is* a
`DocumentObject` with properties: it uses core's property system, the recompute DAG and
documents. So `ferrocad_part -> ferrocad_core`, and no extraction is needed to make that
edge work.

**"Do we need a `ferrocad_types`?"** Yes, but not for Part-vs-core. It is needed for the
**seam**: the OCCT backend must implement the geometry trait *without* depending on
core's document model, and the trait needs value types (`Placement`, `Vector3`,
`Quantity`) that currently live in core. Without extraction the edge would invert
(`geom -> core`), dragging documents into the backend crate.

So the extraction is narrow: the **value types** (`quantity`, `unit`, and
`geometry::{Vector3, Matrix4, Rotation, Placement}`) move to `ferrocad_types`, and core
re-exports them so nothing above notices.

## 2. The seam (from `geometry-and-topology.md` §6, made concrete)

A **`ferrocad_geom`** leaf crate holds the kernel-independent vocabulary and the trait:

```rust
pub struct Shape(/* opaque, kernel-owned handle */);
pub struct ElementRef { pub shape: Shape, pub name: String }
pub struct History { pub generated: Vec<(ElementRef, ElementRef)>, /* modified, deleted */ }
pub struct ElementMap { /* stable name -> current sub-shape */ }

pub trait GeometryBackend {
    type Error;
    fn make_box(&self, l: f64, w: f64, h: f64) -> Result<Shape, Self::Error>;
    fn fuse(&self, a: &Shape, b: &Shape) -> Result<(Shape, History), Self::Error>;
    fn fillet(&self, s: &Shape, edges: &[ElementRef], r: f64) -> Result<(Shape, History), Self::Error>;
    fn resolve(&self, s: &Shape, name: &str) -> Option<Shape>;
    // ... one method per operation a workbench actually calls
}
```

Two implementations are in scope, exactly as §6 proposed:

- a **`NullBackend`** in `ferrocad_geom` (placeholder shapes, empty maps) so documents,
  persistence and their tests need no OCCT; and
- **`ferrocad_occt`**, the real one.

## 3. `ferrocad_occt`: the backend crate

It owns everything OCCT-specific, so no other crate has to:

- depends on `opencascade-sys` for geometry, and on the **sibling bridge** (the
  `#[cxx::bridge]` from the history spike) for history/element maps — **no fork, no
  patch** of `opencascade-sys`;
- implements `ferrocad_geom::GeometryBackend`, and ports FreeCAD's `FCBRepAlgoAPI_*`
  behaviour (auto-fuzzy scaled to model bounds, non-destructive, recursive compounds)
  that `occt-history-spike.md` found is required for a usable element map;
- wraps `TopoDS_Shape` in the single `ferrocad_geom::Shape` newtype, so one owner (Rust)
  manages the handle (see `geometry-and-topology.md` §7.5).

It is a **leaf above `ferrocad_geom`** — it does not depend on `ferrocad_core`.

## 4. Injection: one backend, installed at startup

`ferrocad_core` (or the `App`) holds `Arc<dyn GeometryBackend>` and defaults to
`NullBackend`. The application installs the real one once:

```
App::init()  ->  set_geometry_backend(Arc::new(OcctBackend::new()))
```

Features ask the document for the backend (`doc.geometry()`), the same way FreeCAD
features reach `App::GetApplication()`. Nothing below the app names `ferrocad_occt`, so:

- the headless engine and its tests run with `NullBackend`;
- swapping or mocking a kernel is one call, not a recompile of core.

## 5. Dependency rules (the edges to keep)

```
ferrocad_types  -> (crates.io only)                        [leaf: Quantity, Placement, ...]
ferrocad_geom   -> ferrocad_types                          [leaf: Shape, History, traits]
ferrocad_occt   -> ferrocad_geom, ferrocad_types, opencascade-sys   [kernel backend; no core]
ferrocad_core   -> ferrocad_types, ferrocad_geom           [still a leaf wrt OCCT]
ferrocad_part   -> ferrocad_core, ferrocad_geom            [workbench logic; no occt]
ferrocad_part_py-> ferrocad_part, ferrocad_occt, pyo3      [wires the backend, module `Part`]
ferrocad_py     -> ferrocad_core, pyo3                     [module `ferrocad`; no Part]
ferrocad (app)  -> ferrocad_gpui, ferrocad_py,
                   ferrocad_part_py, ferrocad_occt         [installs the backend]
```

Two optional extras are deliberately out:

- `ferrocad_part` does **not** depend on `ferrocad_occt`, so it can be unit-tested
  against `NullBackend` and never forces the kernel to build.
- `ferrocad_py` does **not** depend on `ferrocad_part_py`, so the core bindings stay
  Part-free; the app links both and registers both as built-in modules.

## 6. Reuse by other workbenches

Because the trait is in `ferrocad_geom` and the backend in `ferrocad_occt`, another
workbench (PartDesign, Draft, an edition) that needs the kernel directly depends on
**those two**, not on Part. In practice it should keep using the single installed
backend from the `App` rather than constructing its own — one kernel instance, one set of
shape handles — but the layout does not forbid a second, isolated `OcctBackend` (useful
for a background import/export worker).

## 7. Staged rollout

The extraction is a refactor with its own tests, so it lands in stages:

1. **[x] `ferrocad_types`** (done 2026-10-06): the value types (`quantity`, `unit`,
   `geometry`) moved out of `ferrocad_core`, which now depends on and re-exports them, so
   the public surface is unchanged.
2. **[x] `ferrocad_geom`** (done 2026-10-06): `Shape`, `History`, `ElementRef`,
   `ElementMap`, `OpResult`, `GeometryBackend` and `NullBackend`. It depends only on
   `ferrocad_types`; `ferrocad_core` does not depend on it yet. (The doc's temporary
   `geom -> core` edge was avoided: `ferrocad_types` landed first.)
3. **[x] `ferrocad_occt`** (done 2026-10-06): implements `GeometryBackend` via
   `opencascade-sys` + the sibling bridge (`include/fc_history.hxx`). First operations:
   `make_box`, `fuse`, `cut`, `fillet`, `place`, and a positional `resolve`; fuse/cut
   build a face-level `History`. Verified against OCCT 7.8.1 (5 tests + doctest) and
   tested in CI by a `geometry` job that fetches OCCT from conda-forge. **Not yet
   installed in the app**: that needs the `App` backend slot below.
4. **`ferrocad_part` / `ferrocad_part_py`**: Part features as `DocumentObject`s, exposed
   as the Python module `Part`.

`ferrocad_occt` is deliberately **not** in `default-members`, so an ordinary
`cargo build`/`cargo test` needs no kernel; only the `geometry` CI job (and geometry
work) sets `OCCT_INCLUDE_DIR` / `OpenCASCADE_DIR` / `LD_LIBRARY_PATH` from a fetched
OCCT prefix.

Naming follows the existing snake_case crates (`ferrocad_core`, `ferrocad_py`); the
Python-visible names stay `ferrocad` and `Part`.
