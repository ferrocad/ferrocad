# The document half round — 2026-11-15

Status: round record. This is the measurement of the P5 work that made the base app and
the toy edition literally kernel-free. The backlog is
[`../TODO-15-11-2026.md`](../TODO-15-11-2026.md) §P5; the dependency rules and the
contract are [`../../../contracts/editions.json`](../../../contracts/editions.json) and
[`../../developer/testing.md`](../../developer/testing.md); the crate map is
[`../../developer/architecture.md`](../../developer/architecture.md).

## 1. The problem

`486a985` (the S9 composition) made `ferrocad_shell_python` depend on
`ferrocad_shell_rust`, which linked `ferrocad_part` and OCCT. Every edition that embeds
the host therefore reached the kernel, so `cargo xtask editions` could only assert the
weaker rule "not through a dependency of its own" — the `COMPOSED_DOCUMENT_HALF`
carve-out. The documented promise ("`cargo install ferrocad` is a clean build with no
kernel", `docs/developer/architecture/repackaging.md` §4a) was not true of the graph.

## 2. The split

The document backend turned out to be almost entirely kernel-free. Only four points
needed a kernel:

1. registering the object types a decoded document may name;
2. building the startup sample;
3. tessellating an object's stored shape;
4. constructing the two Part feature edits a manipulator commit asks for (the edge
   dress-up and the face push/pull).

Everything else — the graph, the property rows, undo, `.FCStd` read/write, the model
tree, the stage, the command catalogue, the provider registry, the settings cards — is
`ferrocad_core` + `ferrocad_shell_api`. So the split is those four points behind one
trait:

```text
ferrocad_shell_doc  (new, kernel-free)
  DocumentBackend   the former backend, with `engine: Box<dyn DocumentEngine>`
  DocumentEngine    register_types / sample / mesh / dress_up / push_pull
  NoEngine          the default: an empty sample, no mesh, features refused by name

ferrocad_shell_rust (now thin)
  PartEngine        implements DocumentEngine over ferrocad_part + ferrocad_occt
  PartEngine::backend() / ::shared_backend()
  re-exports DocumentBackend
```

`ferrocad_shell_python` composes `ferrocad_shell_doc` as a normal dependency and
`ferrocad_shell_rust` as an **optional** one behind a new `part` feature. `ferrocad`
forwards it (`part = ["ferrocad_shell_python/part"]`); `ferrocad_parametric` and
`ferrocad_sculpt` depend on `ferrocad = { features = ["part"] }`. The base app, the toy
edition and the browser composition are unchanged in behaviour:

- the desktop boot has always been the *script* half's document (the Rust sample is only
  built when the registry is still empty after the app script boots), so a kernel-free
  base backs an empty document until the script makes one;
- the toy edition draws through its script's `stage_meshes`, which is exactly the
  path the kernel-free backend leaves to the script half;
- the browser host keeps `ferrocad_shell_rust` (it compiles OCCT to wasm by design).

## 3. Measured

| Gate | Before | After |
| --- | --- | --- |
| `cargo xtask editions --check` | green with the carve-out | green with the **strict** rule |
| `ferrocad` closure | reached `ferrocad_part`, `ferrocad_occt`, `ferrocad_shell_rust` | none of them; `ferrocad_shell_doc` instead |
| `ferrocad_toy` closure | same leak | same fix |
| `ferrocad_shell_doc` unit tests | — | **54 passed** (the backend's own suite, with a fixture engine) |
| `ferrocad_shell_rust` integration tests | — | **6 passed** (`tests/part_backend.rs`: the sample, tessellation, the three feature edits, undo through the registry) |
| `ferrocad_shell_python` | 24 + 9 | **24 + 9** (no `part` feature) |
| `ferrocad_shell_core` | 289 | **289** (kernel-free engine) |
| `ferrocad_toy` | 6 | **6** (one stale document name fixed, §5) |
| `ferrocad_shell_web` | 8 | **8** |
| `ferrocad_parametric` (`--no-default-features`) | 33 | **33** (with `part`) |
| `ferrocad_sculpt` (`--no-default-features`) | 9 | **9** (with `part`) |

## 4. Decisions

- **One trait, boxed at construction.** The engine is `Box<dyn DocumentEngine>` rather
  than a type parameter so `DocumentBackend` is one type in both builds: `engine()`,
  `compose_all` and the browser's `plain()` return the same type whether or not `part`
  is on. The alternative (a generic `DocumentBackend<E>`) would have forced every
  caller of `engine()` to name the engine type.
- **The trait's defaults are the kernel-free answer.** `mesh` returns `None`, and the
  two feature edits return an error naming the missing Part workbench, so an engine
  implements only what it can do (`NoEngine` implements just `sample`).
- **The Part-specific tests moved with the engine.** Four backend tests read Part
  geometry or feature types; they are `tests/part_backend.rs` in
  `ferrocad_shell_rust` now. The seven that merely needed an object that *recomputes*
  stayed in the document half, against a two-object test engine (`Test::Box` + a
  dependent `Test::Fillet`).
- **`RustAppBackend` is retired as a name.** The type is
  `ferrocad_shell_doc::DocumentBackend`, re-exported by `ferrocad_shell_rust`; the
  constructors are `PartEngine::backend()` / `::shared_backend()`. Keeping an alias
  whose `new()` silently meant *kernel-free* would have been a trap.

## 5. What the verification found

- `ferrocad_toy`'s `round_trip` test closed the bootstrap document by the name `Toy`,
  while the shell's startup sample names it `Sample`; it had been failing since
  `a35d4de` made the base `bootstrap` ignore its `doc_name` argument. The test now
  closes the active document by the name it reports.
- `cargo xtask editions --write` changed only the base/toy closures (part and OCCT out,
  `ferrocad_shell_doc` in) and added `ferrocad_shell_doc` to the host and edition
  closures. `--check` then passed with no carve-out.

## 6. What this does not do

- It does not move the *scripting* halves; `ferrocad_shell_python` still embeds CPython
  and still links libpython. The split is about the geometry engine, not the interpreter.
- It does not make the browser kernel-free. `ferrocad_web_gpui` compiles OCCT to wasm on
  purpose; the contract never claimed otherwise.
- It does not change `ferrocad_part`'s or `ferrocad_occt`'s own dependency shape.
