// The wasm bundle ABI, run under Node against the real `ferrocad_addon_toy.wasm`.
//
// The host doorway is faked here on purpose: this test is about the boundary — memory,
// packing, allocation, and that the bundle speaks the real JSON protocol (`create`,
// `set`, `properties`, `document`) — not about the document, which `ferrocad_doc_api`'s
// own tests cover. The fake answers the same envelope the real one does.
//
//     cargo build -p ferrocad_addon_toy --release --target wasm32-unknown-unknown
//     node --test crates/ferrocad_web_gpui/www/addons/loader.test.mjs

import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";

import { instantiate } from "./loader.js";

const WASM =
  process.env.FERROCAD_ADDON ??
  new URL(
    "../../../../target/wasm32-unknown-unknown/release/ferrocad_addon_toy.wasm",
    import.meta.url,
  );

/** A doorway that behaves like `ferrocad_doc_api::DocApi` for the verbs a bundle uses. */
function host() {
  const documents = [];
  let nextEntity = 0;
  const named = (name) => documents.find((object) => object.name === name);
  const ok = (result) => JSON.stringify({ ok: true, version: 1, result });
  const no = (error) => JSON.stringify({ ok: false, version: 1, error });

  return (request) => {
    const call = JSON.parse(request);
    switch (call.call) {
      case "document":
        return ok({ name: "Sample", objects: documents });
      case "properties": {
        const object = named(call.object);
        return object ? ok(object.properties) : no(`no object ${call.object}`);
      }
      case "create": {
        const object = {
          name: call.name,
          label: call.name,
          type_id: call.type,
          entity: ++nextEntity,
          additional_type_ids: call.type.split("::"),
          driven: [],
          properties: Object.entries(call.properties ?? {}).map(([name, spec]) => ({
            name,
            type_id: spec.type,
            kind: "quantity",
            value: spec.value,
            choices: [],
          })),
        };
        documents.push(object);
        return ok({
          name: object.name,
          properties: Object.keys(call.properties ?? {}),
          recomputed: 1,
        });
      }
      case "set": {
        const object = named(call.object);
        if (!object) return no(`no object ${call.object}`);
        const row = object.properties.find((property) => property.name === call.property);
        if (!row) return no(`${call.object} has no property ${call.property}`);
        row.value = call.value;
        return ok({ object: call.object, property: call.property, value: call.value });
      }
      default:
        return no(`unknown call ${call.call}`);
    }
  };
}

async function bundle() {
  return instantiate(await readFile(WASM), host());
}

const height = (mesh) => Math.max(...mesh.positions.map((point) => point[2]));

test("the bundle registers its commands", async () => {
  const addon = await bundle();
  assert.deepEqual(
    JSON.parse(addon.commands()).map((command) => command.id),
    ["Toy_Slab", "Toy_Taller"],
  );
});

test("a command crosses the import and writes the document", async () => {
  const doorway = host();
  const addon = await instantiate(await readFile(WASM), doorway);

  // An id that is not this bundle's is refused without touching the document.
  assert.equal(addon.run("Someone_Else"), false);
  assert.deepEqual(JSON.parse(doorway('{"call":"document"}')).result.objects, []);

  assert.equal(addon.run("Toy_Slab"), true);
  const document = JSON.parse(doorway('{"call":"document"}')).result;
  assert.equal(document.objects.length, 1);
  assert.equal(document.objects[0].name, "Toy");
  assert.equal(document.objects[0].type_id, "App::FeaturePython");
  assert.equal(document.objects[0].properties[0].name, "Height");
  assert.equal(document.objects[0].properties[0].value, "20 mm");
});

test("the display hook's mesh crosses back intact", async () => {
  const addon = await bundle();

  // Nothing to draw before the object exists.
  assert.equal(addon.stageMeshes("[]"), "[]");

  assert.equal(addon.run("Toy_Slab"), true);
  const [mesh] = JSON.parse(addon.stageMeshes("[]"));
  assert.equal(mesh.object, "Toy");
  assert.equal(mesh.entity, 1);
  assert.equal(mesh.display, "shaded_edges");
  assert.equal(mesh.positions.length, 24);
  assert.equal(mesh.triangles.length, 36);
  assert.equal(mesh.triangle_parts.length, 12);
  assert.equal(mesh.segment_parts.length, 24);
  assert.equal(height(mesh), 20);

  // A second command edits the same object, and the hook reads the new height.
  assert.equal(addon.run("Toy_Taller"), true);
  const [taller] = JSON.parse(addon.stageMeshes("[]"));
  assert.equal(height(taller), 40);

  // The `have` rule: the host says it already meshed the object.
  assert.equal(addon.stageMeshes('["Toy"]'), "[]");
});
