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
needs. It is not a full binding that is missing; it is a handful of history
functions. The pragmatic move is a small **bridge addition**, not writing our own
binding from scratch.

## Reading real history: `patch/0001-history-bridge.patch`

`patch/0001-history-bridge.patch` is a concrete, apply-able patch against
`opencascade-sys` 0.3.0. It adds one header and three bridge functions:

- new `include/fc_history.hxx`: `fc_brep_{fuse,cut,common}_history(op, sub_shape, ...)`,
  which fill `Modified`/`Generated`/`Deleted` (from `BRepBuilderAPI_MakeShape`) and
  `BRepTools_History`'s `Modified`/`Generated`/`Removed` (`-1` when the op keeps no
  history object). It uses out-parameters, so `Handle(BRepTools_History)` never has to
  cross the cxx boundary.
- `src/b_rep_algo_api.rs`: the three matching declarations in the existing bridge.

That is the whole diff: one header plus three functions, reusing the crate's existing
`TopoDS_Shape`/`TopTools_ListOfShape` types.

Apply it one of three ways:

```sh
# A. quick local (dirty, for a spike): patch the extracted crate, then re-run
SYS=$(find ~/.cargo/registry/src -maxdepth 1 -name 'opencascade-sys-0.3.0' | head -1)
patch -p1 -d "$SYS" < patch/0001-history-bridge.patch
cargo run --release --features patched

# B. vendor: cargo vendor, apply the same patch, wire .cargo/config.toml
# C. fork opencascade-rs, apply the same patch, point [patch.crates-io] at the fork
#    (the clean path, and the one that could be sent upstream as a PR)
```

With `--features patched` the run prints real numbers, e.g.:

```
== fuse history ==
fuse history[box a]: Modified=2 Generated=1 Deleted=0 | BRepTools_History: Modified=2 Generated=1 Removed=0
```

Without the feature it prints that history is unavailable, which is the stock-crate
baseline. Element-level iteration (which output index each input maps to) is the next
addition; the counts here are enough to prove the history crosses the boundary.

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
