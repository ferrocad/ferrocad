// FerroCAD · a JS workbench bundle (spike).
//
// This is the browser's answer to `mods/Toy`: it declares commands, owns an object the
// document has no kernel for, and **draws it** through the display hook. Everything it
// touches goes through `window.ferrocad.call(request)` — the JSON doorway the host
// installed from `ferrocad_doc_api::DocApi`. It never links `ferrocad_core`: a second
// copy of the engine in the page would be a second document (and a second kernel).
//
// The two channels a workbench has, both exercised here:
//   * `runCommand(id)`  — a command that creates or edits the object;
//   * `stageMeshes(have)` — the display hook: mesh JSON for an object the document
//     cannot tessellate (here, `App::FeaturePython`, which has no `Shape`).
//
// `install()` runs before the wasm module boots, so the shell's first `commands()`
// read already contains this bundle's ids.

const TOY = "Toy";
const HEIGHT = 20; // mm, the slab's initial height
const FOOTPRINT = 20; // mm per side

const commands = [
  {
    id: "Toy_Slab",
    label: "Toy Slab",
    tooltip: "Create (or reset) the 20 × 20 mm toy slab",
  },
  {
    id: "Toy_Taller",
    label: "Toy Taller",
    tooltip: "Raise the toy slab to 40 mm",
  },
];

// `App::FeaturePython` has no Rust behaviour: the document shows it in the tree and
// never meshes it, which is exactly the case the display hook exists for. Its label
// defaults to its name, so only the height is declared.
const SLAB = {
  call: "create",
  name: TOY,
  type: "App::FeaturePython",
  properties: {
    Height: {
      type: "App::PropertyLength",
      value: `${HEIGHT} mm`,
      group: "Base",
      doc: "Height of the slab",
    },
  },
};

export function install() {
  const page = window.__ferrocadJs ?? {};
  window.__ferrocadJs = {
    ...page,
    active: () => true,
    commands: () => JSON.stringify([...parsePageCommands(page), ...commands]),
    evaluate: page.evaluate ?? ((code) => `toy: ${code}`),
    runCommand: (id) => {
      if (id === "Toy_Slab") return makeSlab(HEIGHT);
      if (id === "Toy_Taller") return makeSlab(40);
      return page.runCommand ? page.runCommand(id) : false;
    },
    stageMeshes: (have) => stageMeshes(parseJson(have, []), page.stageMeshes),
    onEvent: page.onEvent ?? (() => {}),
    // One line per document event, so a headless capture can assert that the host
    // announced an edit and its recompute.
    onDocumentEvent: (id, payloadJson) => {
      console.log(`ferrocad toy: ${id} ${payloadJson}`);
      if (page.onDocumentEvent) page.onDocumentEvent(id, payloadJson);
    },
  };
  console.log("ferrocad toy: registered", commands.map((command) => command.id).join(", "));
}

// Merge with a page surface that was already installed (`?js=1`).
function parsePageCommands(page) {
  return page.commands ? parseJson(page.commands(), []) : [];
}

function parseJson(text, fallback) {
  try {
    return JSON.parse(text);
  } catch {
    return fallback;
  }
}

// --- the doorway ----------------------------------------------------------

function call(request) {
  const doorway = window.ferrocad;
  if (!doorway) {
    return { ok: false, error: "no document doorway" };
  }
  return parseJson(doorway.call(JSON.stringify(request)), {
    ok: false,
    error: "the doorway answered something that is not JSON",
  });
}

function makeSlab(heightMm) {
  // The command is repeatable: create the object once, then edit it.
  const existing = call({ call: "properties", object: TOY });
  if (existing.ok) {
    return edit(TOY, "Height", `${heightMm} mm`);
  }
  const created = call(SLAB);
  if (!created.ok) {
    console.error("ferrocad toy: create failed:", created.error);
    return false;
  }
  if (heightMm !== HEIGHT) {
    return edit(TOY, "Height", `${heightMm} mm`);
  }
  console.log("ferrocad toy: created", created.result.name);
  return true;
}

function edit(object, property, value) {
  const answer = call({ call: "set", object, property, value });
  if (!answer.ok) {
    console.error(`ferrocad toy: ${object}.${property} failed:`, answer.error);
    return false;
  }
  console.log(`ferrocad toy: ${object}.${property} = ${answer.result.value}`);
  return true;
}

// --- the display hook -----------------------------------------------------

// The objects the document already meshed are skipped: re-tessellating a shape the
// backend has would copy every vertex through this boundary for nothing.
function stageMeshes(have, fallThrough) {
  const document = call({ call: "document" });
  if (!document.ok || !document.result) return "[]";
  const object = document.result.objects.find((candidate) => candidate.name === TOY);
  if (!object || have.includes(object.name)) {
    return fallThrough ? fallThrough(JSON.stringify(have)) : "[]";
  }
  return JSON.stringify([slabMesh(object, heightOf(object))]);
}

function heightOf(object) {
  const row = object.properties.find((property) => property.name === "Height");
  const value = row ? parseFloat(row.value) : NaN;
  return Number.isFinite(value) && value > 0 ? value : HEIGHT;
}

// A flat-shaded box standing on the origin plane, `height` millimetres tall.
///
/// Six quads with their own vertices and one normal each, so the shading is flat
/// without needing a normal-average pass. `entity` is the pick id the doorway reported
/// for the object, which is what makes a click resolve to `Toy`.
function slabMesh(object, height) {
  const half = FOOTPRINT / 2;
  const corners = [
    [-half, -half, 0], [half, -half, 0], [half, half, 0], [-half, half, 0],
    [-half, -half, height], [half, -half, height], [half, half, height], [-half, half, height],
  ];
  // Counter-clockwise from outside, so the normal points away from the body.
  const faces = [
    { normal: [0, 0, -1], corners: [0, 3, 2, 1] },
    { normal: [0, 0, 1], corners: [4, 5, 6, 7] },
    { normal: [0, -1, 0], corners: [0, 1, 5, 4] },
    { normal: [1, 0, 0], corners: [1, 2, 6, 5] },
    { normal: [0, 1, 0], corners: [2, 3, 7, 6] },
    { normal: [-1, 0, 0], corners: [3, 0, 4, 7] },
  ];
  const positions = [];
  const normals = [];
  const triangles = [];
  const edges = [];
  for (const face of faces) {
    const base = positions.length;
    for (const corner of face.corners) {
      positions.push(corners[corner]);
      normals.push(face.normal);
    }
    triangles.push(base, base + 1, base + 2, base, base + 2, base + 3);
    // The face's own outline; shared edges are drawn twice, at the same depth.
    edges.push(base, base + 1, base + 1, base + 2, base + 2, base + 3, base + 3, base);
  }
  return {
    entity: object.entity,
    object: object.name,
    label: object.label,
    positions,
    normals,
    triangles,
    triangle_parts: new Array(triangles.length / 3).fill(0),
    edges,
    segment_parts: new Array(edges.length / 2).fill(0),
    parts: ["Slab"],
    color: [0.78, 0.55, 0.30, 1.0],
    display: "shaded_edges",
  };
}
