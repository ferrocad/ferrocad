# Spring

A sandboxed **wasm** workbench: a parametric helical spring.

The document holds three numbers on an `App::FeaturePython` object — `Radius`, `Pitch` and
`Turns` — and **no shape at all**. The bundle reads them through the doorway, computes the
helix itself, and answers the display hook with the mesh. Editing a value in the property
pane and recomputing redraws it, because the hook rereads the document.

| command | what it does |
| --- | --- |
| `Spring_Insert` | create `Spring` with its three declared parameters |
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

It computes geometry without linking an engine — the doorway is its only import, and the
sandbox's fuel budget bounds a runaway. `crates/ferrocad_addon_spring` is the source; its
unit tests drive the same logic over a real `DocApi`.
