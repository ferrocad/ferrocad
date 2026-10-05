# Spike: OCCT from Rust with a real binding crate

The Rust rewrite of [`../occt-history/probe.cpp`](../occt-history/probe.cpp),
using the existing [`opencascade-sys`](https://crates.io/crates/opencascade-sys)
crate (bschwind/opencascade-rs). No hand-rolled binding.

Same question: **does shape history survive into Rust?** The answer turns out to be
about the *crate's coverage*, and it is the important finding.

## Install

OCCT **>= 7.8** dev libraries plus a C++ toolchain and CMake. On Debian/Ubuntu:

```sh
sudo apt-get install -y \
  cmake clang pkg-config \
  libocct-foundation-dev \
  libocct-modeling-data-dev \
  libocct-modeling-algorithms-dev \
  libocct-data-exchange-dev \
  libocct-ocaf-dev \
  libocct-visualization-dev \
  occt-misc
```

That covers every toolkit `opencascade-sys` links (`TKernel`, `TKMath`, `TKBRep`,
`TKTopAlgo`, `TKPrim`, `TKBO`, `TKBool`, `TKFillet`, `TKOffset`, `TKShHealing`,
`TKGeomBase`, `TKGeomAlgo`, `TKG2d`, `TKG3d`, `TKDE`, `TKDESTEP`, `TKDEIGES`,
`TKDESTL`, `TKXSBase`, `TKCAF`, `TKLCAF`, `TKXCAF`, `TKMesh`). The crate finds OCCT
by running `find_package(OpenCASCADE)`; a system install is enough, no env vars.
If you cannot install system-wide, the crate's `builtin` feature builds OCCT from
source instead (slow, needs network).

Then:

```sh
cargo run --release
```

## What the crate actually exposes (the finding)

Verified against the `opencascade-sys` 0.3.0 sources:

| Need | Covered? |
| --- | --- |
| Primitives, booleans, fillet/chamfer | ✅ |
| `TopExp::MapShapes`, `TopoDS` downcasts, `TopTools_IndexedMapOfShape::{Extent,FindKey}` | ✅ |
| `BRepAlgoAPI_Cut::Generated` | ✅ (only this one) |
| `BRepAlgoAPI_Fuse::{Modified,Generated}` | ❌ not bridged |
| `BRepBuilderAPI_MakeShape::{Modified,Generated,IsDeleted}` | ❌ |
| `BRepTools_History`, `BRepAlgoAPI_BuilderAlgo::History` | ❌ |

So the binding builds geometry but cannot hand us the lineage a stable element map
needs. It is not a full hand-rolled binding that is missing; it is a handful of
history functions. The pragmatic move is a small **bridge addition** (either
upstream, or a thin patch/fork) declaring the missing calls, not writing our own
binding from scratch. Sketch:

```rust
// our own #[cxx::bridge], reusing the crate's types, added on top:
pub fn BRepAlgoAPI_Fuse_Modified<'a>(
    self: Pin<&'a mut BRepAlgoAPI_Fuse>, shape: &'a TopoDS_Shape,
) -> &'a TopTools_ListOfShape;
pub fn BRepBuilderAPI_MakeShape_IsDeleted(
    self: &BRepBuilderAPI_MakeShape, shape: &TopoDS_Shape,
) -> bool;
// ...and bridge BRepTools_History / BRepAlgoAPI_BuilderAlgo::History.
```

There is also a milder signal in the crate's own model: the high-level
`BooleanShape` carries `new_edges` (the edges the boolean generated), which is a
coarse, operation-specific form of exactly the lineage we need. It shows the crate
authors already think in these terms.

## Note on the high-level crate

`opencascade` (the high-level crate) wraps `Shape { inner: UniquePtr<TopoDS_Shape> }`
but keeps `inner` `pub(crate)`, so you cannot get the raw `TopoDS_Shape` to call the
sys-level traversal functions. This spike therefore uses `opencascade-sys` directly.

## Not a workspace member

`Cargo.toml` has an empty `[workspace]` table so the FerroCAD workspace never builds
this crate. It is throwaway.
