# Spike: OCCT from Rust (shape history)

The Rust rewrite of [`../occt-history/probe.cpp`](../occt-history/probe.cpp). Same
experiment, same question: **does shape history survive the Rust <-> OCCT boundary,
in a form we can build an element map from?**

## Why a hand-rolled C ABI, not a bindings crate

OCCT is C++ with no C ABI. Two ways to reach it from Rust:

| Approach | Pro | Con (for this spike) |
| --- | --- | --- |
| Existing crate (`opencascade-rs`, `occt-sys`, …) | Less glue to write | Coverage of **history** is unknown and unverifiable offline; history is the whole point, and stock wrappers may expose only the geometry, not `Modified`/`Generated`/`IsDeleted` |
| Hand-rolled C ABI (this spike) | We control exactly which OCCT calls cross the boundary; it doubles as the first sketch of `ferrocad_geom_occt` | More glue (a small `shim.cpp` + `extern "C"` declarations) |

Since history is the crux and we cannot check a crate's coverage without network,
the spike binds the handful of calls by hand. The shim is ~120 lines; the Rust side
is the probe. If a crate later turns out to expose history well, it can replace the
shim behind the same seam.

## The shim surface

`shim/occt_shim.{h,cpp}` exposes opaque `OcctShape`/`OcctHistory` handles and:

- `occt_make_box`, `occt_fuse`, `occt_fillet` (each returns a shape + history);
- `occt_shape_count(shape, kind)`;
- `occt_history_{modified,generated,deleted}(history, input, kind, ordinal)`.

The OCCT classes behind it (`BRepAlgoAPI_Fuse`, `BRepFilletAPI_MakeFillet`,
`BRepBuilderAPI_MakeShape::Modified/Generated/IsDeleted`, `TopExp::MapShapes`) are
the same ones FreeCAD's element mapper consumes.

## Build and run

Requires OCCT **7.8+** dev headers and libraries.

```sh
# Debian/Ubuntu:  sudo apt-get install -y libocct-*-dev
# conda:          conda install -c conda-forge occt
cargo run --release
# If OCCT is not in the default prefix:
#   OCCT_INCLUDE_DIR=/opt/occt/include OCCT_LIB_DIR=/opt/occt/lib cargo run --release
```

**Untested in the authoring sandbox**: that environment has no OCCT, no network, and
`sudo` requires a password, so this crate was never compiled there. The OCCT calls
mirror the verified C++ probe; the Rust/`cc` wiring is written against `cc` 1.x.

## What to look for

Same as the C++ probe (see the report). The Rust-specific question is whether the
`-1`/count protocol carries enough information, or whether the shim needs to return
richer history (the full `BRepTools_History` graph) for a real element map. If the
`Modified`/`Generated` counts come back meaningful, the seam is viable; if the shim
has to grow a bespoke history serialization, that is the design pressure the
`GeometryEngine` trait must absorb.

## Not a workspace member

`Cargo.toml` has an empty `[workspace]` table so the FerroCAD workspace never builds
this crate. It is throwaway.
