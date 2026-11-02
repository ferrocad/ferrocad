// Node smoke test for the browser module (no DOM, no GPU).
//
// Run after `wasm-bindgen --target web --out-dir www/pkg <artifact>.wasm`:
//
//   node www/smoke.mjs
//
// It checks the things that can regress silently: the shipped module must not
// import `wasi_snapshot_preview1` (the link-time stubs in `src/wasi.rs`), the
// `Part` document must recompute through the kernel (a box with one filleted edge
// tessellates to 40 triangles), a fillet the kernel refuses must come back as
// a reported failure rather than aborting the process, and `BRepFeat_SplitShape`
// must actually run -- that is the one operation whose toolkit (`TKFeat`) the
// prebuilt kernel omits, so it is the only way to see that the archive built by
// `run.sh` is really linked.
import { readFile } from "node:fs/promises";
import {
  initSync,
  set_fillet_radius,
  split_check,
  triangles,
} from "./pkg/ferrocad_web.js";

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

// `BRepFeat_SplitShape` is the one kernel operation in the backend whose toolkit
// the prebuilt kernel prunes, so this is the only behavioural proof that the
// `TKFeat` archive `run.sh` builds is linked and usable.
let split;
try {
  split = split_check();
} catch (error) {
  console.error("split_check failed:", error);
  process.exit(1);
}
const [direct, left] = split;
console.log("split:", direct, "direct,", left, "left");
if (direct !== 1 || left !== 1) {
  console.error("expected one direct and one left piece from the face split");
  process.exit(1);
}
