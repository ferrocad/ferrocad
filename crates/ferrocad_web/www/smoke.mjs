// Node smoke test for the browser module (no DOM, no GPU).
//
// Run after `wasm-bindgen --target web --out-dir www/pkg <artifact>.wasm`:
//
//   node www/smoke.mjs
//
// It checks the two things that can regress silently: the shipped module must not
// import `wasi_snapshot_preview1` (the link-time stubs in `src/wasi.rs`), and the
// kernel must actually run (a filleted box tessellates to 40 triangles).
import { readFile } from "node:fs/promises";
import { initSync, triangles } from "./pkg/ferrocad_web.js";

const bytes = await readFile(new URL("./pkg/ferrocad_web_bg.wasm", import.meta.url));
const module = new WebAssembly.Module(bytes);

const wasi = WebAssembly.Module.imports(module).filter((entry) =>
  entry.module.includes("wasi"),
);
if (wasi.length > 0) {
  console.error("unexpected WASI imports:", wasi);
  process.exit(1);
}

initSync({ module });
const count = triangles();
console.log("triangles:", count);
if (count !== 40) {
  console.error("expected 40 triangles from a filleted box");
  process.exit(1);
}
