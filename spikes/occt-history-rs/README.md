# Spike: OCCT from Rust with a real binding crate

The Rust rewrite of [`../occt-history/probe.cpp`](../occt-history/probe.cpp),
using the existing [`opencascade-sys`](https://crates.io/crates/opencascade-sys)
crate (bschwind/opencascade-rs). No hand-rolled binding.

Same question: **does shape history survive into Rust?** The answer turns out to be
about the *crate's coverage*, and it is the important finding.

**Status: verified.** The example compiles and runs against real OCCT 7.8.1, and the
patched build reads real history (see "Verified results" below).

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

## Getting OCCT (two ways)

`opencascade-sys` finds OCCT with `find_package(OpenCASCADE)`, so it needs an OCCT
**7.8+** install with a CMake config. Two ways to get one, neither needing root:

### A. Prebuilt, rootless, minutes (what this spike was verified with)

Use `micromamba` and conda-forge, pinned to 7.8.1:

```sh
# from this directory (spikes/occt-history-rs)
mkdir -p .mm && curl -sSL https://micro.mamba.pm/api/micromamba/linux-64/latest \
  | tar -xj -C .mm
export MAMBA_ROOT_PREFIX=$PWD/.mm/root
.mm/bin/micromamba create -y -p $PWD/.occt78 -c conda-forge occt=7.8.1
export OpenCASCADE_DIR=$PWD/.occt78/lib/cmake/opencascade
export LD_LIBRARY_PATH=$PWD/.occt78/lib:$LD_LIBRARY_PATH   # conda OCCT is shared
```

Conda-forge's unpinned `occt` is now **8.0.1**, which the crate rejects (it requires
major 7). Pin `occt=7.8.1`.

### B. From source via the crate's `builtin` feature (slow)

`occt-sys` bundles the OCCT 7.8.1 sources, so the `builtin` feature builds OCCT
statically with no install at all:

```sh
CMAKE_BUILD_PARALLEL_LEVEL=4 cargo build --release --features builtin
```

This is **not** the fast path: OCCT is ~5700 source files, and on a 4-core / 7 GB box
it swaps and runs for hours. Use method A unless you specifically want the static
build.

### CMake 4.x note

OCCT 7.8.1 (and even the crate's tiny finder project) use a
`cmake_minimum_required` below 3.5, which CMake **4.x** removed. With a new CMake you
must export:

```sh
export CMAKE_POLICY_VERSION_MINIMUM=3.5
```

CMake honors this from the environment, so no source edit is needed.

## Build and run

```sh
. ../.toolchain/env.sh                                 # project-local Rust
export CMAKE_POLICY_VERSION_MINIMUM=3.5
export OpenCASCADE_DIR=$PWD/.occt78/lib/cmake/opencascade

# stock crate: geometry works, history is unavailable
cargo build --release
LD_LIBRARY_PATH=$PWD/.occt78/lib ./target/release/occt-history-rs
```

To read real history, apply the patch and force a rebuild of the crate (Cargo treats
registry sources as immutable and will otherwise reuse the cached build):

```sh
SYS=$(find "$CARGO_HOME/registry/src" -maxdepth 2 -name 'opencascade-sys-0.3.0' | head -1)
patch -p1 -d "$SYS" < patch/0001-history-bridge.patch
cargo clean -p opencascade-sys --release
cargo build --release --features patched
LD_LIBRARY_PATH=$PWD/.occt78/lib ./target/release/occt-history-rs
```

Other application methods: `cargo vendor`, or fork `opencascade-rs` and point
`[patch.crates-io]` at the fork (the clean path, and the one that could go upstream as
a PR).

## Verified results (OCCT 7.8.1, Linux, clang 21, CMake 4.2)

Stock (`cargo build --release`):

```
box a: 1 solids, 6 faces, 12 edges, 8 vertices
box b: 1 solids, 6 faces, 12 edges, 8 vertices
fuse history[box a]: not exposed by stock opencascade-sys (...)
fused: 1 solids, 14 faces, 28 edges, 16 vertices
filleted: 1 solids, 15 faces, 31 edges, 18 vertices
BRepAlgoAPI_Cut::Generated(a) -> 0 shape(s)
```

Patched (`--features patched`):

```
fuse history[box a solid]: Modified=0 Generated=0 Deleted=1 | BRepTools_History: Modified=0 Generated=0 Removed=1
fuse history[box a faces (6)]: Modified=8 Generated=0 Deleted=1 | BRepTools_History: Modified=8 Generated=0 Removed=1
fuse history[box b solid]: Modified=0 Generated=0 Deleted=1 | BRepTools_History: Modified=0 Generated=0 Removed=1
fuse history[box b faces (6)]: Modified=8 Generated=0 Deleted=1 | BRepTools_History: Modified=8 Generated=0 Removed=1
```

### What those numbers mean (a second finding)

Querying the **whole input solid** gives `Modified=0 Generated=0 Deleted=1`: the fuse
consumes both solids, so at the solid level every input is simply "deleted". That
number is useless for naming.

Querying each **face** of the input gives `Modified=8` (6 input faces of box A map to 8
faces of the fused result; one input face is deleted). *This* is the granularity an
element map needs. So the lineage functions are necessary but not sufficient: the next
addition is **element-level iteration** that records, per input sub-shape, which output
sub-shapes it maps to (indices, not just counts). `BRepTools_History` agreeing with the
`BRepBuilderAPI_MakeShape` base on every number is a good sign the bridge is faithful.

## Note on the high-level crate

`opencascade` (the high-level crate) wraps `Shape { inner: UniquePtr<TopoDS_Shape> }`
but keeps `inner` `pub(crate)`, so you cannot get the raw `TopoDS_Shape` to call the
sys-level traversal functions. This spike therefore uses `opencascade-sys` directly.

## Not a workspace member

`Cargo.toml` has an empty `[workspace]` table so the FerroCAD workspace never builds
this crate. It is throwaway.
