// FerroCAD · the page's JS scripting surface (S7 spike).
//
// The page owns the module; the Rust host (`ferrocad_shell_js`) calls these
// functions and the event callback through `window.__ferrocadJs`. The command list
// is read once, when the wasm module boots, so `install()` must run first.

const commands = [
  {
    id: "Demo_JS",
    label: "Demo JS",
    tooltip: "Run the page's JS command",
  },
];

export function install() {
  window.__ferrocadJs = {
    active: () => true,
    commands: () => JSON.stringify(commands),
    evaluate(code) {
      console.log("ferrocad js: evaluate", code);
      return `js: ${code}`;
    },
    runCommand(id) {
      console.log("ferrocad js: run", id);
      return true;
    },
    // `entity` is the picked stage entity (0 for a miss); the page could map it to
    // an object name.
    onEvent(kind, x, y, entity) {
      console.log("ferrocad js: event", kind, "entity", entity, "at", x, y);
    },
  };
  console.log(
    "ferrocad js: registered",
    commands.map((command) => command.id).join(", "),
  );
}
