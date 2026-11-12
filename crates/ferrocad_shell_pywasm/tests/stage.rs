//! B18b item 2: the interactive stage under RustPython.
//!
//! `ferrocad_stage` is the browser's face of the same Rust crate `ferrocad_stage_py`
//! wraps on CPython. These tests assert the surface `python/ferrocad_shell` and the
//! `FreeCADGui` facade actually use: the enums compare, a `ShapeItem`/`Stage` round-trip
//! their tessellation, a Python `Tool` subclass drives through the native `Tool`
//! interface, snapping and the published view work, and the ranked picker sees a
//! published stage.

use ferrocad_shell_api::ScriptHost;
use ferrocad_shell_pywasm::PywasmScriptHost;
use ferrocad_stage as fs;
use serde_json::Value;

/// Run `f` with RustPython's 16 MiB stack (its compiler recurses; see `properties.rs`).
fn with_host<R: Send + 'static>(f: impl FnOnce(&PywasmScriptHost) -> R + Send + 'static) -> R {
    std::thread::Builder::new()
        .stack_size(16 << 20)
        .spawn(move || {
            let host = ferrocad_shell_pywasm::new().expect("the interpreter boots");
            f(&host)
        })
        .expect("a worker thread")
        .join()
        .expect("no panic")
}

fn evaluate(host: &PywasmScriptHost, code: &str) -> Value {
    let answer = host.call("evaluate", &[code]).expect("evaluate runs");
    serde_json::from_str(&answer).expect("evaluate returns JSON")
}

/// Run `code`, assert it succeeded, and answer what it printed.
fn output(host: &PywasmScriptHost, code: &str) -> String {
    let answer = evaluate(host, code);
    assert_eq!(answer["ok"], true, "{code}: {answer}");
    answer["output"].as_str().unwrap_or_default().to_string()
}

#[test]
fn the_module_is_importable_on_both_paths() {
    with_host(|host| {
        let text = output(
            host,
            "import ferrocad_stage as s\n\
             from ferrocad import stage as alias\n\
             print(s.__version__, alias is s)\n",
        );
        let mut parts = text.split(' ');
        assert!(parts.next().is_some_and(|version| !version.is_empty()));
        assert_eq!(parts.next(), Some("True"), "{text}");
    });
}

#[test]
fn the_enum_variants_compare_by_value() {
    with_host(|host| {
        let text = output(
            host,
            "import ferrocad_stage as s\n\
             print(s.PointerKind.Down == s.PointerKind.Down, \
                   s.PointerKind.Down != s.PointerKind.Up, \
                   s.MouseButton.Left == s.MouseButton.Right, \
                   s.DrawMode.Wireframe == s.DrawMode.Wireframe)\n",
        );
        assert_eq!(text, "True True False True", "{text}");
    });
}

#[test]
fn a_shape_item_round_trips_its_tessellation() {
    with_host(|host| {
        let text = output(
            host,
            "import ferrocad_stage as s\n\
             item = s.ShapeItem(7)\n\
             item.set_shape([[0, 0, 0], [1, 0, 0], [0, 1, 0]], [], [0, 1, 2], [0, 1, 1, 2, 2, 0])\n\
             item.draw_mode = s.DrawMode.Wireframe\n\
             item.surface_color = (1.0, 0.0, 0.0, 1.0)\n\
             item.selectable = False\n\
             print(item.entity_id, len(item.positions), len(item.triangles), \
                   len(item.edges), item.draw_mode == s.DrawMode.Wireframe, \
                   item.surface_color, item.selectable)\n",
        );
        assert_eq!(text, "7 3 3 6 True (1.0, 0.0, 0.0, 1.0) False", "{text}");
    });
}

#[test]
fn a_stage_compiles_its_items() {
    with_host(|host| {
        let text = output(
            host,
            "import ferrocad_stage as s\n\
             item = s.ShapeItem(9)\n\
             item.set_shape([(0, 0, 0), (1, 0, 0), (0, 1, 0)], [], [0, 1, 2], [0, 1, 1, 2, 2, 0])\n\
             stage = s.Stage()\n\
             stage.add(item)\n\
             print(len(stage), stage.count(), stage.is_empty(), stage.batch_count(), stage.entities())\n",
        );
        assert_eq!(text, "1 1 False 1 [9]", "{text}");
    });
}

#[test]
fn a_workplane_and_snap_context_resolve_a_point() {
    with_host(|host| {
        let text = output(
            host,
            "import ferrocad_stage as s\n\
             plane = s.Workplane.world()\n\
             context = s.SnapContext(plane, 1.0, 10.0)\n\
             snap = context.snap_point((0.4, 0.6, 0.0), [], 0.0)\n\
             print(snap.kind == s.SnapKind.Grid, snap.point, snap.major)\n",
        );
        assert_eq!(text, "True (0.0, 1.0, 0.0) False", "{text}");
    });
}

#[test]
fn the_published_view_round_trips_previews_and_picks() {
    with_host(|host| {
        let text = output(
            host,
            "import ferrocad_stage as s\n\
             s.clear_view()\n\
             camera = s.Camera.orthographic(10.0, (800, 600), 1.0, 100.0)\n\
             s.set_view_camera(camera)\n\
             item = s.ShapeItem(7)\n\
             item.set_shape([(-1, -1, 0), (1, -1, 0), (0, 1, 0)], [], [0, 1, 2], [])\n\
             stage = s.Stage()\n\
             stage.add(item)\n\
             s.set_view_stage(stage, {7: 'Tri'})\n\
             hits = s.pick_at(400, 300)\n\
             print(s.view_camera().size, len(hits), hits[0].object, hits[0].entity_id)\n",
        );
        assert_eq!(text, "(800, 600) 1 Tri 7", "{text}");
    });
}

#[test]
fn a_python_tool_drives_through_the_native_interface() {
    with_host(|host| {
        output(
            host,
            "\
import ferrocad_shell, ferrocad_stage
class Probe(ferrocad_stage.Tool):
    def __init__(self):
        self.seen = 0
        self.add_item(ferrocad_stage.ShapeItem(7))
    def on_event(self, event):
        self.seen += 1
        return True
    def status(self):
        return {'id': 'probe', 'label': 'Probe', 'active': True, 'points': self.seen, 'hint': 'h'}
    def cancel(self):
        return True
_probe = Probe()
ferrocad_shell.tool_bridge = lambda name: _probe
ferrocad_shell.tool_command = lambda name, verb: True
",
        );

        let mut tool = host.tool("probe").expect("the bridge builds a tool");
        assert_eq!(tool.items().len(), 1, "the tool's item is visible");
        assert_eq!(tool.status().points, 0);

        let event = fs::PointerEvent {
            kind: fs::PointerKind::Down,
            position: [10, 20],
            button: Some(fs::MouseButton::Left),
            hit: None,
            projection: None,
        };
        tool.on_event(&event, None);
        assert_eq!(tool.status().points, 1, "the event reached Python");
        assert!(tool.cancel(), "cancel forwards to the Python method");
        assert!(
            host.tool_command("probe", "cancel"),
            "the verb reaches the bridge"
        );
    });
}

#[test]
fn a_raising_tool_is_reported_not_propagated() {
    with_host(|host| {
        output(
            host,
            "\
import ferrocad_shell, ferrocad_stage
class Bad(ferrocad_stage.Tool):
    def on_event(self, event):
        raise RuntimeError('boom')
_bad = Bad()
ferrocad_shell.tool_bridge = lambda name: _bad
",
        );

        let mut tool = host.tool("bad").expect("the bridge builds a tool");
        let event = fs::PointerEvent {
            kind: fs::PointerKind::Move,
            position: [1, 2],
            button: None,
            hit: None,
            projection: None,
        };
        tool.on_event(&event, None);
        assert!(tool.items().is_empty());
        assert!(!tool.status().active);
    });
}

#[test]
fn the_draft_bridge_is_the_shells_own_tool() {
    with_host(|host| {
        let text = output(
            host,
            "import ferrocad_shell\n\
             bridge = ferrocad_shell.tool_bridge('draft')\n\
             print(type(bridge).__name__, bridge.status()['id'])\n",
        );
        assert_eq!(text, "DraftTool draft", "{text}");
        // The shell can build it through the same path a command uses.
        assert!(host.tool("draft").is_ok());
    });
}
