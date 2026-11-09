// The fastener library: ISO 4762 socket-head cap screws as real `Part::Cylinder`s.
//
// A JS workbench is an ES module exporting `install()`. The host evaluates it before the
// window boots and reads `window.__ferrocadJs` once, so this merges into whatever another
// package installed instead of replacing it.
//
// Every write goes through `window.ferrocad.call` — the JSON doorway — which is the same
// document CPython and the viewport reach. A host that refuses the write answers
// `{"ok": false, …}`, and the command reports that instead of pretending it ran.

const SIZES = [
  { id: "Fastener_M3x10", label: "M3 × 10", diameter: 3, length: 10 },
  { id: "Fastener_M4x16", label: "M4 × 16", diameter: 4, length: 16 },
  { id: "Fastener_M5x20", label: "M5 × 20", diameter: 5, length: 20 },
  { id: "Fastener_M6x20", label: "M6 × 20", diameter: 6, length: 20 },
  { id: "Fastener_M8x30", label: "M8 × 30", diameter: 8, length: 30 },
];

const byId = new Map(SIZES.map((size) => [size.id, size]));

/** One screw: a cylinder with the shank's radius and length. */
function insert(size) {
  const doorway = window.ferrocad;
  if (!doorway) {
    console.error("fastener: no document doorway on this host");
    return false;
  }
  const part = size.id.slice("Fastener_".length);
  const answer = JSON.parse(
    doorway.call(
      JSON.stringify({
        call: "create",
        name: `ISO4762_${part}`,
        type: "Part::Cylinder",
        properties: {
          // A cylinder's `Radius` and `Height` are lengths, so the unit is part of the
          // value the editor would type.
          Radius: `${size.diameter / 2} mm`,
          Height: `${size.length} mm`,
        },
      }),
    ),
  );
  if (!answer.ok) {
    console.error("fastener:", answer.error);
    return false;
  }
  return true;
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
