//! `pivy.coin` on RustPython: the Coin workbenches' surface, the overlay bridge, and
//! `ViewObject.RootNode`.
//!
//! These are the calls `mods/GridOverlayCoin`, `mods/AxisOverlayCoin` and
//! `mods/GizmoOverlayShader` make, plus the one `ferrocad_py`'s `RootNode` makes. The
//! rules behind them are `ferrocad_inventor`'s (`fields`, `class_name`, `view_root`,
//! `overlays`); this file pins that the RustPython adapter reaches them.

use ferrocad_core::Document;
use ferrocad_graphics::Topology;
use ferrocad_inventor::overlays;
use ferrocad_shell_pywasm::PywasmScriptHost;
use serde_json::Value;

/// The overlay registry and the document registry are process state; one test at a time.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|error| error.into_inner())
}

/// Run `f` with RustPython's 16 MiB stack (its compiler recurses).
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

const GRID_TREE: &str = r#"
from pivy import coin
import FerroCADGui

switch = coin.SoSwitch()
switch.whichChild = coin.SO_SWITCH_NONE
root = coin.SoSeparator()
pick = coin.SoPickStyle()
pick.style = coin.SoPickStyle.UNPICKABLE
root.addChild(pick)
coords = coin.SoCoordinate3()
coords.point.setValues(0, 3, [(0.0, 0.0, 0.0), (10.0, 0.0, 0.0), (0.0, 10.0, 0.0)])
face = coin.SoFaceSet()
face.numVertices.setValue(3)
root.addChild(coords)
root.addChild(face)
switch.addChild(root)
switch.whichChild = coin.SO_SWITCH_ALL
print(switch.getNumChildren(), root.getNumChildren(), coords.point.getNum(), isinstance(switch, coin.SoNode))
FerroCADGui.addOverlay(switch)
"#;

#[test]
fn a_script_builds_a_coin_tree_and_the_overlay_bridge_draws_it() {
    let _guard = serial();
    overlays::clear();
    with_host(|host| {
        let text = output(host, GRID_TREE);
        assert_eq!(text, "1 3 3 True", "{text}");

        // The bridge is the *core* registry the shell drains, and the extracted tree is
        // the one the compiler walks: one triangle from the three coordinates.
        assert!(overlays::dirty(), "addOverlay marked the set dirty");
        let scenes = overlays::take_scenes();
        assert_eq!(scenes.len(), 1, "one overlay");
        let (queue, _directives) = ferrocad_inventor::compile(&scenes[0]);
        assert_eq!(queue.len(), 1, "one packet from the face set");
        assert_eq!(
            queue.iter().next().map(|packet| packet.topology),
            Some(Topology::Triangles)
        );
        assert!(!overlays::dirty(), "taking the scenes clears the flag");
    });
}

#[test]
fn an_overlay_is_removed_by_identity() {
    let _guard = serial();
    overlays::clear();
    with_host(|host| {
        let text = output(
            host,
            r#"
from pivy import coin
import FerroCADGui
node = coin.SoSeparator()
other = coin.SoSeparator()
FerroCADGui.addOverlay(node)
FerroCADGui.addOverlay(other)
print(FerroCADGui.removeOverlay(node), FerroCADGui.removeOverlay(node))
"#,
        );
        assert_eq!(text, "True False", "{text}");
        assert_eq!(overlays::take_scenes().len(), 1, "only `other` is left");
    });
}

#[test]
fn the_coin_nodes_the_workbenches_use_keep_their_fields() {
    let _guard = serial();
    with_host(|host| {
        let text = output(
            host,
            r#"
from pivy import coin

depth = coin.SoDepthBuffer()
depth.set("write FALSE")
light = coin.SoLightModel()
light.model = coin.SoLightModel.BASE_COLOR
binding = coin.SoMaterialBinding()
binding.value = coin.SoMaterialBinding.PER_PART

vertex = coin.SoVertexShader()
vertex.source = "@vertex fn vs() {}"
fragment = coin.SoFragmentShader()
program = coin.SoShaderProgram()
program.shaderObject.set1Value(0, vertex)
program.shaderObject.set1Value(1, fragment)

color = coin.SoShaderParameter3f()
color.name = "grid_color"
color.value = (0.2, 0.4, 0.6)
spacing = coin.SoShaderParameter1f()
spacing.name = "base_spacing"
spacing.value = 2.5
shadows = coin.SoShaderParameterArray1f()
shadows.name = "shadow_data"
shadows.value.setValues(0, 3, [0.0, 1.0, 2.0])
shadows.value.setNum(3)

material = coin.SoMaterial()
material.diffuseColor = [(1.0, 0.0, 0.0), (0.0, 1.0, 0.0)]

print(depth.write, depth.function == coin.SoDepthBuffer.LESS)
print(light.model == coin.SoLightModel.BASE_COLOR, binding.value == coin.SoMaterialBinding.PER_PART)
print(vertex.source, len(fragment.source), program.shaderObject.getNum())
print(color.name, round(color.value[0], 2), round(color.value[2], 2), round(spacing.value, 2), shadows.value.getNum())
print(material.diffuseColor.getNum(), material.diffuseColor.getValues()[1])
print(coin.SoSeparator().getNumChildren(), coin.SoSeparator.AUTO, coin.SO_SWITCH_ALL)
"#,
        );
        assert_eq!(
            text,
            "False True\n\
             True True\n\
             @vertex fn vs() {} 0 2\n\
             grid_color 0.2 0.6 2.5 3\n\
             2 (0.0, 1.0, 0.0, 1.0)\n\
             0 2 -3",
            "{text}"
        );
    });
}

/// `ViewObject.RootNode` is the shared body (`ferrocad_inventor::view_root`) behind the
/// RustPython adapter: two display representations, and a tree a script can hang nodes on.
#[test]
fn the_root_node_is_the_shared_view_tree() {
    let _guard = serial();
    {
        let mut registry = ferrocad_core::application()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        for name in registry.names() {
            registry.close_document(&name);
        }
        registry.insert_document("Root", Document::new());
    }

    std::thread::Builder::new()
        .stack_size(16 << 20)
        .spawn(|| {
            let host = ferrocad_shell_pywasm::live().expect("the interpreter boots");
            let text = output(
                &host,
                r#"
import Part
from pivy import coin
print(Part.makeBox(1, 1, 1).ShapeType)
box = App.ActiveDocument.addObject('Part::Box', 'Box')
box.ViewObject.ShapeColor = (1.0, 0.0, 0.0, 1.0)
root = box.ViewObject.RootNode
print(isinstance(root, coin.SoNode), root.getNumChildren())
root.addChild(coin.SoSeparator())
print(root.getNumChildren())
"#,
            );
            assert_eq!(text, "Solid\nTrue 2\n3", "{text}");
        })
        .expect("a worker thread")
        .join()
        .expect("no panic");
}
