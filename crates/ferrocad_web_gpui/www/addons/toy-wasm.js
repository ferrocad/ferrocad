// The wasm toy bundle, installed into the page's `window.__ferrocadJs`.
//
// `loader.js` instantiates `toy.wasm` and provides the `ferrocad.call` import; this
// module only merges the bundle's commands and display hook into the surface
// `JsScriptHost` already drives. `install()` runs before the wasm host boots (so the
// first `commands()` read includes the bundle) and awaits the module fetch, which is
// why `index.html` awaits it.

import { instantiate } from "./loader.js";

const WASM = new URL("./toy.wasm", import.meta.url);
// The capability the bundle ships with (`run.sh` copies the crate's manifest here).
const MANIFEST = new URL("./toy.json", import.meta.url);

let bundle = null;

/** `read` unless the manifest grants writes; a missing manifest is read-only. */
async function capability() {
  try {
    const response = await fetch(MANIFEST);
    if (!response.ok) return "read";
    const manifest = await response.json();
    return manifest.document === "read-write" || manifest.document === "write"
      ? "read-write"
      : "read";
  } catch {
    return "read";
  }
}

export async function install() {
  const bytes = await (await fetch(WASM)).arrayBuffer();
  const document = await capability();
  bundle = await instantiate(bytes, (request) => {
    // The host installs `window.ferrocad` when it boots; a command cannot run before
    // that, so this is only ever called once the document API exists.
    const api = window.ferrocad;
    return api
      ? api.call(request)
      : JSON.stringify({ ok: false, error: "no document API" });
  }, { document });

  const page = window.__ferrocadJs ?? {};
  const commands = JSON.parse(bundle.commands());
  window.__ferrocadJs = {
    ...page,
    active: () => true,
    commands: () => JSON.stringify([...pageCommands(page), ...commands]),
    evaluate: page.evaluate ?? ((code) => `toy: ${code}`),
    // Log like the JS bundle does: a silently unrun command in a headless capture
    // is otherwise indistinguishable from a command that never reached the bundle.
    runCommand: (id) => {
      const ran = bundle ? bundle.run(id) : false;
      if (ran) console.log(`ferrocad toy (wasm): run ${id}`);
      return ran || (page.runCommand ? page.runCommand(id) : false);
    },
    stageMeshes: (have) => (bundle ? bundle.stageMeshes(have) : "[]"),
    onEvent: page.onEvent ?? (() => {}),
    // One line per document event, so a headless capture can assert that the host
    // announced an edit and its recompute.
    onDocumentEvent: (id, payloadJson) => {
      console.log(`ferrocad toy (wasm): ${id} ${payloadJson}`);
      if (page.onDocumentEvent) page.onDocumentEvent(id, payloadJson);
    },
  };
  console.log("ferrocad toy (wasm): registered", commands.map((command) => command.id).join(", "));
  // Observable in a headless capture: which capability the manifest granted.
  console.log(`ferrocad toy (wasm): document ${document}`);
}

// Merge with a page surface that was already installed (`?js=1`).
function pageCommands(page) {
  if (!page.commands) return [];
  try {
    return JSON.parse(page.commands());
  } catch {
    return [];
  }
}
