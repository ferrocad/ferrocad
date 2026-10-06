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

pub trait GeometryBackend: Send + Sync {
    // object-safe: errors are boxed (GeomError), not an associated type
    fn make_box(&self, l: f64, w: f64, h: f64) -> Result<Shape, GeomError>;
    fn fuse(&self, a: &Shape, b: &Shape) -> Result<OpResult, GeomError>;
    fn fillet(&self, s: &Shape, edges: &[ElementRef], r: f64) -> Result<OpResult, GeomError>;
    fn resolve(&self, s: &Shape, name: &str) -> Option<Shape>;
    fn save_shape(&self, s: &Shape) -> Vec<u8>;   // kernel owns the encoding (OCCT: BREP)
    fn load_shape(&self, bytes: &[u8]) -> Option<Shape>;
    // ... one method per operation a workbench actually calls
}
```

The trait is used as a **trait object** (`Arc<dyn GeometryBackend>`), so it cannot carry
an associated `Error` type. Each backend keeps its own concrete error enum and boxes it
at the seam (a `GeomError = Box<dyn Error + Send + Sync>` alias in `ferrocad_geom`).

[`Shape`](https://docs.rs/ferrocad_geom) is `Send` but **not** `Sync`. It shares its payload
by letting the kernel clone its own handle (a separate `Arc<()>` token carries `is_same`
identity) rather than putting the payload behind an `Arc`, which would force `T: Sync`.
So a `Send`-only kernel handle such as OCCT's `TopoDS_Shape` sits in `OcctShape` directly,
with no `Mutex`; exclusivity comes from the document's own lock. The reason the bound
matters: `#[pyclass]` types must be `Send + Sync`, so a bare extension value must never be
a field of one.

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

## 4. Where the kernel lives: Part, not core

Geometry does **not** belong in `ferrocad_core`, and there is no `Application` backend
slot. Upstream agrees: `App` core contains no `TopoDS` at all (`grep -l TopoDS src/App/*.h`
is empty); the OCCT shape lives in `Mod/Part/App/PropertyTopoShape.h`, and `Part::Feature`
is a Part class. `App` is the kernel-agnostic document model; **Part is the kernel.**

So:

- `ferrocad_core` stays geometry-free. A Part feature is a core `DocumentObject`, but the
  *shape it holds* is a Part property type, not a core one.
- **`ferrocad_part` owns the kernel.** The library depends only on the seam; a
  `OnceLock<Arc<dyn GeometryBackend>>` is filled by `ferrocad_part::register(backend)`, which
  the application calls once with the kernel it wants. Part defines `Part::Feature` and holds
  the shape property, but never names a kernel type. Core never sees it.
- **Composition is compile-time.** "Injection" in a Rust app is which crates the edition
  links (and which Cargo features are on), not a runtime slot on a core singleton:

  | Edition app | links | kernel |
  | --- | --- | --- |
  | FerroCAD: Architecture | `ferrocad_part_py` → `ferrocad_occt` | OCCT |
  | a 2D-only Draft app | no Part | none |

  That is exactly the repackaging vision ([`repackaging.md`](repackaging.md)): minimal
  apps that do not include Part link no OCCT, and the shell library (`ferrocad_gpui`,
  formerly `ferrocad_host`) never does. The runtime `Arc<dyn GeometryBackend>` is how Part
  obtains the kernel — the edition app calls `ferrocad_part::register(Arc::new(OcctBackend::new()))`
  at startup, and tests call it with `NullBackend` — but it lives in Part, not core.

## 5. The prerequisite: a document-object SPI

There is a catch, and it is bigger than the backend slot. Today `ferrocad_core` cannot
host a Part feature or a shape property, because it has **no extension point**:

- `Property` is a **closed enum** (`property.rs`); a module cannot add a variant.
- `DocumentObject` is a **concrete struct**; recompute calls a built-in `execute_object`,
  not a trait method a module supplies. Its `extensions` is a `BTreeSet<String>` of
  *names* — no behaviour.
- `typeregistry::default_properties` is a closed `match type_id`, not a registry.

Upstream does not have this problem: `App::Property` is a base class that modules
subclass (`Part::PropertyPartShape`), `Part::Feature` subclasses `App::Feature`, and both
register with `Base::Type`. Porting Part therefore needs core to grow the same kind of
seam **before any shape property exists**:

- **Object types** — a `DocumentObject` *behaviour* seam plus a registry, so
  `ferrocad_part` can register `Part::Feature` (construction, `execute`, property schema).
- **Property types** — a property seam with the hooks core's persistence needs (at minimum
  `save`/`restore`, ideally `copy`/`execute`), so a Part shape property can serialise BREP
  itself. This is what the closed enum most clearly cannot express: `save_to_file` has to
  delegate persistence to the property, and cannot serialise a kernel handle on its own.

The exact shape of that seam (trait objects vs. a registry keyed by `Base::Type`-style
names; how Python `Proxy` objects fit) is its own design note. The point here: it is the
real next prerequisite for Part — not an OCCT backend slot in core.

## 6. Dependency rules (the edges to keep)

```
ferrocad_types   -> (crates.io only)                       [leaf: Quantity, Placement, ...]
ferrocad_geom    -> ferrocad_types                         [leaf: Shape, History, traits]
ferrocad_occt    -> ferrocad_geom, ferrocad_types, opencascade-sys   [kernel impl]
ferrocad_core    -> ferrocad_types                          [geometry-agnostic; never geom/occt]
ferrocad_part    -> ferrocad_core, ferrocad_geom, ferrocad_types     [workbench; kernel injected]
ferrocad_part_py -> ferrocad_part, ferrocad_occt, ferrocad_py, pyo3  [module `Part`; composition root]
ferrocad_py      -> ferrocad_core, pyo3                     [module `ferrocad`; no Part]
ferrocad (app)   -> ferrocad_gpui, ferrocad_py,
                    ferrocad_part_py                       [edition composition]
```

`ferrocad_core` does **not** depend on `ferrocad_geom`: the shape handle lives in a Part
property, so core never names a kernel type. This is the change from the earlier plan,
which put the backend on `Application` (§4). Two further edges:

- `ferrocad_part` links **no kernel crate**; it holds an injected `Arc<dyn GeometryBackend>`.
  Testing against `NullBackend` or OCCT is done by passing a different backend to
  `ferrocad_part::register`, not by a feature that removes OCCT. `ferrocad_occt` is a
  **dev-dependency** of Part (its own unit tests) and a real dependency of the
  composition root that wires the kernel in (`ferrocad_part_py` / the edition app).
- `ferrocad_py` does **not** depend on `ferrocad_part_py`, so the core bindings stay
  Part-free; the edition app links both and registers both as built-in modules.
- `ferrocad_part_py` **does** depend on `ferrocad_py`, but only for the extension-conversion
  hook (below), never the reverse. That is what breaks the would-be cycle and keeps Part's
  Python module out of the core bindings.

### The extension-conversion hook

A `Property::Extension` value is opaque to core. `ferrocad_py` therefore keeps a small
registry of converters, keyed by registered type name, populated by the owning module at
import: `ferrocad_py::register_extension_converter("Part::PropertyPartShape", to_python,
from_python)`. Without it, `obj.Shape` reads as `None`; with it, it round-trips to and from
`Part.Shape`. This is the same split as upstream, where the value's Python representation
lives in the module that owns the property type.

**Why one image.** The registry is a process-global `OnceLock`, like
`ferrocad_core::application`. Two separate extension modules (`.so` files) each statically
link their own copy of that state, so a standalone `Part.abi3.so` would not see the
documents `import FreeCAD` created. Part must therefore be linked into the same image as
the core bindings — the app does this with `pyo3::append_to_inittab!`; so must the wheel.
`ferrocad_part_py` ships as `rlib` (for that composition) and `cdylib` (for the future
standalone). Wiring it into `crates/ferrocad` and the wheel — which also means bundling
OCCT (see [`occt-bundling.md`](occt-bundling.md)) — is the next packaging step.

## 7. Reuse by other workbenches

Because the trait is in `ferrocad_geom` and the backend in `ferrocad_occt`, another
workbench that needs the kernel (PartDesign, Draft, an edition) depends on **those two**
directly, not on Part. There is no core singleton to consult; each kernel-using module owns
or shares the backend instance it was composed with. Keeping one instance per process is a
convention (and matters for shape-handle identity), not something core enforces.

## 8. Staged rollout

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
   tested in CI by a `geometry` job that fetches OCCT from conda-forge.
4. **[x] `Application` (core document registry)** (done 2026-10-06) —
   `ferrocad_core::Application` owns the open documents
   (`BTreeMap<String, Arc<Mutex<Document>>>`), the active document, document-lifecycle
   observers and FreeCAD-style unique naming; the PyO3 module delegates to it and the
   Python facade keeps no state (only a wrapper-identity cache in the binding).
   Independent of geometry.
5. **[x] Document-object SPI** (done 2026-10-06) — `ferrocad_core::object_registry`
   (`ObjectType`: defaults + `execute`), `ferrocad_core::property_types` (name-keyed
   property registry; `addProperty` raises on an unknown name) and `ferrocad_core::extension`
   (`Property::Extension`: module-owned values with `clone`/`eq`/`save`/`restore`). A module
   can now register a name, an object type, and a value that holds its own data — everything
   Part needs.
6. **[x] `ferrocad_part`** (started 2026-10-06) — registers `Part::Feature` (with `Shape`
   and `Placement`) via `object_registry`, and `Part::PropertyPartShape` (a
   `ShapeProperty` holding a `ferrocad_geom::Shape`, persisted as in-memory BREP **through the
   injected backend**) via `property_types`. Decoupled from `ferrocad_occt` (dev-dependency
   only): the kernel is injected with `ferrocad_part::register(backend)`.
7. **[x] `ferrocad_part_py`** (started 2026-10-06) — the `Part` Python module. It is the
   composition root: it links `ferrocad_occt`, calls `ferrocad_part::register`, and exposes
   `Part.makeBox` and `Part.Shape`. It registers the `Part::PropertyPartShape` ↔ `Part.Shape`
   converter with `ferrocad_py`, so `obj.Shape` round-trips (verified by
   `tests/part_boot.rs`, which embeds CPython with both modules built in). Wiring it into the
   app binary and the wheel is the remaining packaging step (see §6).

`ferrocad_occt` is deliberately **not** in `default-members`, so an ordinary
`cargo build`/`cargo test` needs no kernel; only the `geometry` CI job (and geometry
work) sets `OCCT_INCLUDE_DIR` / `OpenCASCADE_DIR` / `LD_LIBRARY_PATH` from a fetched
OCCT prefix.

Naming follows the existing snake_case crates (`ferrocad_core`, `ferrocad_py`); the
Python-visible names stay `ferrocad` and `Part`.
