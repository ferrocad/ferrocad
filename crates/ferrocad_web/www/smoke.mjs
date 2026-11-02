// Node smoke test for the browser module (no DOM, no GPU).
//
// Run after `wasm-bindgen --target web --out-dir www/pkg <artifact>.wasm`:
//
//   node www/smoke.mjs
//
// It checks the things that can regress silently: the shipped module must not
// import `wasi_snapshot_preview1` (the link-time stubs in `src/wasi.rs`), the
// `Part` document must recompute through the kernel (a box with one filleted edge
// tessellates to 40 triangles), and a fillet the kernel refuses must come back as
// a reported failure rather than aborting the process.
import { readFile } from "node:fs/promises";
import { initSync, set_fillet_radius, triangles } from "./pkg/ferrocad_web.js";

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

// The original bug: an OCCT refusal escaped as an uncaught C++ exception and
// aborted the process. It has to be a value now, and the feature keeps the last
// good shape so the host can still draw it.
const refused = set_fillet_radius(12);
if (refused.ok || !refused.error) {
  console.error("expected the 12 mm fillet to be refused, got", refused.ok, refused.error);
  process.exit(1);
}
if (refused.faces !== 7 || refused.triangles !== 40) {
  console.error("the refused rebuild must keep the last good shape", refused.faces, refused.triangles);
  process.exit(1);
}
console.log("refused:", refused.error);

const accepted = set_fillet_radius(2);
if (!accepted.ok) {
  console.error("expected the 2 mm fillet to build:", accepted.error);
  process.exit(1);
}
console.log("rebuilt:", accepted.faces, "faces,", accepted.triangles, "triangles");
