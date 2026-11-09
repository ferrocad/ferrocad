// The explode view: spread the model's objects out, and put them back.
//
// A view operation, not a model edit: the addon writes `Placement` through the document
// API (`window.ferrocad.call`) and remembers the strings it replaced, so collapsing restores
// exactly what was there rather than dividing by a factor and drifting.
//
// The rotation is carried through as text: a placement reads `(x, y, z)` or
// `(x, y, z; ax, ay, az; angle)`, and this only rewrites the three base numbers.

const EXPLODE = new Map(); // object name -> the placement string before the explode
const RADIUS = 40; // mm from the origin for the first ring
const RING = 8; // objects per ring

const COMMANDS = [
  {
    id: "Explode_Out",
    label: "Explode",
    tooltip: "Spread the objects out from the origin",
  },
  {
    id: "Explode_In",
    label: "Collapse",
    tooltip: "Put every object back where it was",
  },
];

/** The document API, or `null` when this host has none. */
function documentApi() {
  const api = window.ferrocad;
  if (!api) {
    console.error("explode: no document API on this host");
    return null;
  }
  return api;
}

/** One JSON call, in and out. */
function call(request) {
  const api = documentApi();
  if (!api) return { ok: false, error: "no document API" };
  return JSON.parse(api.call(JSON.stringify(request)));
}

/** `(x, y, z)` / `(x, y, z; ax, ay, az; angle)` → base numbers and the rotation tail. */
function splitPlacement(text) {
  const trimmed = String(text).trim();
  if (!trimmed.startsWith("(") || !trimmed.endsWith(")")) return null;
  const [base, ...rest] = trimmed.slice(1, -1).split(";");
  const values = base.split(",").map((value) => Number(value.trim()));
  if (values.length !== 3 || values.some((value) => Number.isNaN(value))) {
    return null;
  }
  return { base: values, tail: rest.length > 0 ? `;${rest.join(";")}` : "" };
}

/** A number as the property text wants it: no exponent, no trailing zeros. */
function number(value) {
  return String(Number(value.toFixed(6)));
}

function placementText(base, tail) {
  return `(${base.map(number).join(", ")}${tail})`;
}

/** The objects that can move, in document order. */
function movable() {
  const answer = call({ call: "document" });
  if (!answer.ok) {
    console.error("explode:", answer.error);
    return [];
  }
  return (answer.result.objects ?? []).filter((object) =>
    (object.properties ?? []).some((row) => row.name === "Placement"),
  );
}

function placementOf(object) {
  return object.properties.find((row) => row.name === "Placement");
}

function setPlacement(name, value) {
  const answer = call({
    call: "set",
    object: name,
    property: "Placement",
    value,
  });
  if (!answer.ok) {
    console.error("explode:", answer.error);
  }
  return answer.ok === true;
}

function explodeOut() {
  const objects = movable();
  if (objects.length === 0) return false;

  let moved = 0;
  objects.forEach((object, index) => {
    const parsed = splitPlacement(placementOf(object).value);
    if (!parsed) return;

    // Remember the first position only: a second Explode keeps the original, so Collapse
    // always returns to the state the document was in.
    if (!EXPLODE.has(object.name)) {
      EXPLODE.set(object.name, placementOf(object).value);
    }

    const angle = (2 * Math.PI * index) / objects.length;
    const radius = RADIUS * (1 + Math.floor(index / RING));
    const base = [
      parsed.base[0] + radius * Math.cos(angle),
      parsed.base[1] + radius * Math.sin(angle),
      parsed.base[2],
    ];
    if (setPlacement(object.name, placementText(base, parsed.tail))) {
      moved += 1;
    }
  });
  return moved > 0;
}

function explodeIn() {
  let moved = 0;
  for (const [name, value] of EXPLODE) {
    if (setPlacement(name, value)) {
      moved += 1;
    }
  }
  EXPLODE.clear();
  return moved > 0;
}

export function install() {
  const page = window.__ferrocadJs ?? {};
  const previous = page.commands ? JSON.parse(page.commands()) : [];
  const commands = previous.concat(COMMANDS);

  window.__ferrocadJs = {
    active: page.active ?? (() => true),
    commands: () => JSON.stringify(commands),
    evaluate: page.evaluate ?? ((code) => `js: ${code}`),
    runCommand(id) {
      if (id === "Explode_Out") return explodeOut();
      if (id === "Explode_In") return explodeIn();
      return page.runCommand ? page.runCommand(id) : false;
    },
    stageMeshes: page.stageMeshes ?? (() => "[]"),
    onEvent: page.onEvent,
    onDocumentEvent: page.onDocumentEvent,
  };
}
