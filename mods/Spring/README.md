# Spring

A sandboxed **wasm** workbench: a parametric helical spring.

The document holds four numbers on an `App::FeaturePython` object — `Radius`, `Pitch`,
`Turns` and `Wire` — and **no shape at all**. The bundle reads them through the document
API, sweeps a tube itself, and answers the display hook with the mesh. Editing a value in
the property pane and recomputing redraws it, because the hook rereads the document.

The tube is a circle swept along the helix: positions, a normal per point and triangles.
That is arithmetic, not a kernel, which is why a bundle with no geometry can still show a
solid rather than a line. `Wire` is the tube's diameter, and it is clamped to 0.3 × the
pitch when drawn, so tightening cannot fold the coils through each other.

`Placement` is a real row, and the hook applies it: `App_Move`, the gizmo and Explode all
move an object by writing that property, and one that has no such row is skipped. The mesh
is cached against the values it was built from, because the host asks a display hook again
on every sync — a shapeless object is never in the `have` list — and re-sweeping the tube
each frame would cost about 61 million sandbox fuel.

| command | what it does |
| --- | --- |
| `Spring_Insert` | create `Spring` with its five declared properties |
| `Spring_Tighten` | halve `Pitch`, down to 1 mm |

## The bundle is built, not committed

`addon.wasm` is a `wasm32-unknown-unknown` artifact (it is git-ignored, like the page's
`toy.wasm`). Build it beside this manifest:

```sh
cargo build -p ferrocad_addon_spring --target wasm32-unknown-unknown --release
cp target/wasm32-unknown-unknown/release/ferrocad_addon_spring.wasm mods/Spring/addon.wasm
```

Then a desktop run with `--features addons` finds it: B9b discovers
`mods/<Name>/addon.wasm` without a flag, and the bundle rides beside whatever script half
booted.

## Why this is a wasm addon

It computes geometry without linking an engine — the document API is its only import, and
the sandbox's fuel budget bounds a runaway. The display hook gets the larger budget
(`MESH_FUEL`), because its cost is the mesh it emits; the commands get the small one.
`crates/ferrocad_addon_spring` is the source; its unit tests drive the same logic over a
real `DocApi`.
