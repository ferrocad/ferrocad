// The fastener library: ISO 4762 socket-head cap screws as real `Part::Cylinder`s.
//
// A JS workbench is an ES module exporting `install()`. The host evaluates it before the
// window boots and reads `window.__ferrocadJs` once, so this merges into whatever another
// package installed instead of replacing it.
//
// Every write goes through `window.ferrocad.call` — the JSON API — which is the same
// document CPython and the viewport reach. A host that refuses the write answers
// `{"ok": false, …}`, and the command reports that instead of pretending it ran.
//
// A cap screw is two primitives, not one: the shank, and the head placed on top of it.
// The document API has no boolean verbs (docs/project/tasks/backend-parity.md, D16), so a
// hex socket — a cut into the head — is out of reach. The two cylinders at least give the
// silhouette: a shaft with a head on the end.

const SIZES = [
  // d = shank diameter, L = length under the head, dk = head diameter, k = head height.
  { id: "Fastener_M3x10", label: "M3 × 10", diameter: 3, length: 10, head: 5.5, headHeight: 3 },
  { id: "Fastener_M4x16", label: "M4 × 16", diameter: 4, length: 16, head: 7, headHeight: 4 },
  { id: "Fastener_M5x20", label: "M5 × 20", diameter: 5, length: 20, head: 8.5, headHeight: 5 },
  { id: "Fastener_M6x20", label: "M6 × 20", diameter: 6, length: 20, head: 10, headHeight: 6 },
  { id: "Fastener_M8x30", label: "M8 × 30", diameter: 8, length: 30, head: 13, headHeight: 8 },
];

const byId = new Map(SIZES.map((size) => [size.id, size]));

/** One cylinder, its base at `z`. */
function cylinder(name, radius, height, z) {
  const api = window.ferrocad;
  if (!api) {
    console.error("fastener: no document API on this host");
    return false;
  }
  const properties = {
    // A cylinder's `Radius` and `Height` are lengths, so the unit is part of the
    // value the editor would type.
    Radius: `${radius} mm`,
    Height: `${height} mm`,
  };
  if (z !== 0) {
    // A placement without a rotation keeps whatever rotation the object has, which is
    // none: the head sits square on the shank.
    properties.Placement = `(0, 0, ${z})`;
  }
  const answer = JSON.parse(
    api.call(JSON.stringify({ call: "create", name, type: "Part::Cylinder", properties })),
  );
  if (!answer.ok) {
    console.error("fastener:", answer.error);
    return false;
  }
  return true;
}

/** One screw: the shank, then its head on top. */
function insert(size) {
  const part = size.id.slice("Fastener_".length);
  const name = `ISO4762_${part}`;
  return (
    cylinder(name, size.diameter / 2, size.length, 0) &&
    cylinder(`${name}_head`, size.head / 2, size.headHeight, size.length)
  );
}

export function install() {
  const page = window.__ferrocadJs ?? {};
  const previous = page.commands ? JSON.parse(page.commands()) : [];
  const commands = previous.concat(
    SIZES.map((size) => ({
      id: size.id,
      label: `Insert ${size.label}`,
      tooltip: "ISO 4762 socket head cap screw",
    })),
  );

  window.__ferrocadJs = {
    active: page.active ?? (() => true),
    commands: () => JSON.stringify(commands),
    evaluate: page.evaluate ?? ((code) => `js: ${code}`),
    runCommand(id) {
      if (byId.has(id)) {
        return insert(byId.get(id));
      }
      // Another package's command, if one installed before this one.
      return page.runCommand ? page.runCommand(id) : false;
    },
    stageMeshes: page.stageMeshes ?? (() => "[]"),
    onEvent: page.onEvent,
    onDocumentEvent: page.onDocumentEvent,
  };
}
