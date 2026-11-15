//! Where every command lives: the dock contract, the pill, the wheel and Search.
//!
//! The action dock is **one strip**: `[ Undo  Redo  Fit │ ⊕ <Tool> ▾ │ Search ]`. It is
//! not a toolbar, so a creation tool declares `FerroCAD_Surface: "creation"` (the pill's
//! flyout) and a shape verb declares `"radial"` (a context wheel wedge) or `"palette"`
//! (Search only). This test reads the same `commands()` JSON the shell builds its chrome
//! from, so a command that drifts back onto the dock fails here rather than being found
//! in a screenshot.
//!
//! It needs OCCT, like the other edition round trips.

use std::path::Path;
use std::sync::{Mutex, Once};

use ferrocad_shell_python::python;
use ferrocad_shell_api::commands::TopologyGate;
use pyo3::types::PyAnyMethods;
use serde_json::Value;

/// The tests share one interpreter and one document registry.
static TEST_LOCK: Mutex<()> = Mutex::new(());

static BOOT: Once = Once::new();

fn sibling_checkout() -> Option<std::path::PathBuf> {
    let candidate = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../simplecad");
    candidate
        .is_dir()
        .then(|| candidate.canonicalize().unwrap_or(candidate))
}

fn boot() {
    BOOT.call_once(|| {
        ferrocad_combined_py::register_builtins();
        if let Some(checkout) = sibling_checkout() {
            pyo3::Python::with_gil(|py| {
                let sys = py.import("sys").expect("sys");
                sys.getattr("path")
                    .expect("path")
                    .call_method1("insert", (0, checkout.to_string_lossy().to_string()))
                    .expect("insert the checkout on sys.path");
            });
        }
        python::configure("ferrocad_simplecad", &[], &[], false);
    });
    python::bootstrap().expect("boot the edition");
}

/// Every registered command, keyed by id, exactly as the shell reads it.
fn registry() -> serde_json::Map<String, Value> {
    let result = python::evaluate(
        r#"
import json
import ferrocad_simplecad
entries = json.loads(ferrocad_simplecad.commands())
print(json.dumps({entry["id"]: entry for entry in entries}))
"#,
    )
    .expect("the interpreter ran");
    assert!(result.ok, "{}", result.output);
    match serde_json::from_str::<Value>(result.output.trim()).expect("a registry object") {
        Value::Object(map) => map,
        other => panic!("the registry is not an object: {other}"),
    }
}

fn surface(registry: &serde_json::Map<String, Value>, id: &str) -> String {
    registry
        .get(id)
        .unwrap_or_else(|| panic!("{id} is registered"))["surface"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

fn target(registry: &serde_json::Map<String, Value>, id: &str) -> String {
    registry
        .get(id)
        .unwrap_or_else(|| panic!("{id} is registered"))["target"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

fn category(registry: &serde_json::Map<String, Value>, id: &str) -> String {
    registry
        .get(id)
        .unwrap_or_else(|| panic!("{id} is registered"))["category"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Every id whose surface is exactly `wanted`.
fn with_surface(registry: &serde_json::Map<String, Value>, wanted: &str) -> Vec<String> {
    let mut ids: Vec<String> = registry
        .iter()
        .filter(|(_, entry)| entry["surface"].as_str() == Some(wanted))
        .map(|(id, _)| id.clone())
        .collect();
    ids.sort();
    ids
}

#[test]
fn the_dock_is_the_globals_and_nothing_else() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    let registry = registry();

    // The strict contract: three globals. The pill and Search are shell chrome, built
    // by the dock itself, so they are not registry rows.
    assert_eq!(
        with_surface(&registry, "dock"),
        vec![
            "App_Fit".to_string(),
            "App_Redo".to_string(),
            "App_Undo".to_string()
        ],
        "the action dock holds the globals, never a list of verbs"
    );

    // Every surface is one the shell knows; an unknown value would silently fall back to
    // the dock, which is how the strip overflowed before.
    for (id, entry) in &registry {
        let declared = entry["surface"].as_str().unwrap_or_default();
        if declared.is_empty() {
            continue;
        }
        assert!(
            [
                "grid",
                "axis",
                "workplane",
                "snap",
                "radial",
                "mods",
                "creation",
                "palette",
                "dock",
                "hidden",
                // The gizmo's settings are a capability surface with no chrome of its
                // own: the alias parses to `hidden`.
                "gizmo",
            ]
            .contains(&declared),
            "{id}: unknown surface {declared:?}"
        );
    }

    // The edition's own commands are creation tools, wheel verbs or Search utilities.
    for id in [
        "SimpleCAD_Rectangle",
        "SimpleCAD_Circle",
        "SimpleCAD_Polygon",
        "SimpleCAD_Arc",
        "SimpleCAD_Offset",
        "SimpleCAD_Elongate",
        "SimpleCAD_RotateEdge",
        "SimpleCAD_Shell",
        "SimpleCAD_Draft",
        "SimpleCAD_Heal",
        "Part_Box",
        "Part_Polyline",
        "Part_Chamfer",
        "Part_HealFaces",
        "Part_Compound",
        "Sketch_Polyline",
        "Sketch_Dimension",
    ] {
        assert_ne!(
            surface(&registry, id),
            "dock",
            "{id} is not a dock button"
        );
    }
}

#[test]
fn the_creation_tools_are_pill_rows_with_a_category() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    let registry = registry();

    for id in [
        "SimpleCAD_Rectangle",
        "SimpleCAD_Circle",
        "SimpleCAD_Polygon",
        "SimpleCAD_Arc",
    ] {
        assert_eq!(surface(&registry, id), "creation", "{id}");
        assert_eq!(category(&registry, id), "Sketch", "{id}");
    }
    // The primitive the pill defaults to, and the two drawn profiles.
    assert_eq!(surface(&registry, "Part_Box"), "creation");
    assert_eq!(category(&registry, "Part_Box"), "Solid");
    for id in ["Part_Polyline", "Sketch_Polyline"] {
        assert_eq!(surface(&registry, id), "creation", "{id}");
        assert_eq!(category(&registry, id), "Sketch", "{id}");
    }
    // A verb that arms a shape rather than acting on a pick is a pill tool too.
    for id in ["SimpleCAD_Loft", "SimpleCAD_Sweep"] {
        assert_eq!(surface(&registry, id), "creation", "{id}");
        assert_eq!(category(&registry, id), "Surface", "{id}");
    }
}

#[test]
fn the_shape_verbs_are_wheel_wedges_with_a_context_target() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    let registry = registry();

    for id in [
        "SimpleCAD_Shell",
        "SimpleCAD_Draft",
        "SimpleCAD_FillHole",
        "SimpleCAD_Unwrap",
        "SimpleCAD_Subdivide",
        "Part_HealFaces",
    ] {
        assert_eq!(surface(&registry, id), "radial", "{id}");
        assert!(target(&registry, id).contains("face"), "{id}: {}", target(&registry, id));
    }
    for id in [
        "SimpleCAD_Offset",
        "SimpleCAD_Elongate",
        "SimpleCAD_RotateEdge",
        "SimpleCAD_RemoveRounds",
        "Part_Chamfer",
    ] {
        assert_eq!(surface(&registry, id), "radial", "{id}");
        assert!(target(&registry, id).contains("edge"), "{id}: {}", target(&registry, id));
    }
    for id in ["Part_Fuse", "Part_Cut", "Part_Common"] {
        assert_eq!(surface(&registry, id), "radial", "{id}");
        assert!(target(&registry, id).contains("body"), "{id}: {}", target(&registry, id));
    }
    // Delete is the shell's own verb and stays any-context, so it is the last wedge of
    // every ring; the shell orders it there.
    assert_eq!(surface(&registry, "App_Delete"), "radial");
    assert_eq!(target(&registry, "App_Delete"), "");
}

#[test]
fn the_utilities_are_search_only() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    let registry = registry();

    for id in [
        "Sketch_Dimension",
        "Part_Compound",
        "SimpleCAD_Slice",
        "SimpleCAD_Cleanup",
        "SimpleCAD_Heal",
    ] {
        assert_eq!(surface(&registry, id), "palette", "{id}");
    }
}

/// The gate each command declares, parsed by the same code the chrome uses.
fn gate(registry: &serde_json::Map<String, Value>, id: &str) -> Option<TopologyGate> {
    registry
        .get(id)
        .unwrap_or_else(|| panic!("{id} is registered"))["gate"]
        .as_str()
        .and_then(TopologyGate::parse)
}

/// C1: a creation tool declares the topology it needs, and the gate is the selection's,
/// not the Modify hand's: two profiles satisfy `Loft`, one does not.
#[test]
fn the_gated_tools_declare_what_they_need() {
    use ferrocad_shell_api::commands::ElementKind;

    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    let registry = registry();

    let loft = gate(&registry, "SimpleCAD_Loft").expect("Loft declares a gate");
    assert!(
        !loft.satisfied(1, ElementKind::Body),
        "one profile is not a loft: {}",
        loft.requirement()
    );
    assert!(loft.satisfied(2, ElementKind::Body), "two profiles are");
    assert_eq!(loft.requirement(), "select at least 2 bodies");

    let shell = gate(&registry, "SimpleCAD_Shell").expect("Shell declares a gate");
    assert!(!shell.satisfied(1, ElementKind::Body), "a body is not a face");
    assert!(shell.satisfied(1, ElementKind::Face), "one face is enough");

    let chamfer = gate(&registry, "Part_Chamfer").expect("Chamfer declares a gate");
    assert!(chamfer.satisfied(1, ElementKind::Edge), "one edge is enough");
    assert!(!chamfer.satisfied(1, ElementKind::Face), "a face is not an edge");

    let subdivide = gate(&registry, "SimpleCAD_Subdivide").expect("Subdivide declares a gate");
    assert!(subdivide.satisfied(1, ElementKind::Face));
    assert!(!subdivide.satisfied(2, ElementKind::Face), "one face, not two");

    // A tool with no gate is always offered (the Box is the pill's default).
    assert!(
        gate(&registry, "Part_Box").is_none(),
        "the box needs nothing selected"
    );
}
