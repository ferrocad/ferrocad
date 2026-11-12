//! B18b item 2: scripts build and measure real solids under RustPython.
//!
//! The kernel is OCCT, the same one `ferrocad_shell_rust` hands the document half, so a
//! box built here is the shape the viewport draws. Two paths are covered: standalone
//! `Part`/`Mesh` objects (no document), and the document path — `Part.show`, the typed
//! `obj.Shape = …` write, and a parametric `Part::Box` whose `Shape` recomputes.

use ferrocad_core::Document;
use ferrocad_shell_pywasm::PywasmScriptHost;
use serde_json::Value;

/// The registry is process state; the tests that write a document take this one at a time.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|error| error.into_inner())
}

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

fn clear_registry() {
    let mut registry = ferrocad_core::application()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    for name in registry.names() {
        registry.close_document(&name);
    }
}

#[test]
fn a_script_builds_measures_and_reshapes_solids() {
    with_host(|host| {
        // Primitives, mass properties and topology.
        let text = output(
            host,
            "b = Part.makeBox(10, 20, 30)\n\
             print(b.Volume, b.Area, b.ShapeType, len(b.Faces), len(b.Edges), len(b.Vertexes))\n",
        );
        assert_eq!(text, "6000.0 2200.0 Solid 6 12 8", "{text}");

        // Bounds and the centre of mass come from the kernel, not from the arguments.
        let text = output(
            host,
            "print(b.BoundBox.XMax, b.BoundBox.ZMax, b.CenterOfMass.x, b.CenterOfMass.z)\n",
        );
        assert_eq!(text, "10.0 30.0 5.0 15.0", "{text}");

        // A boolean result is a real solid, and it is smaller than the operand.
        let text = output(
            host,
            "c = Part.makeCylinder(5, 40)\n\
             cut = b.cut(c)\n\
             print(cut.isValid(), cut.Volume < b.Volume, cut.Volume > 0)\n",
        );
        assert_eq!(text, "True True True", "{text}");

        // Translation and rotation bake into the geometry.
        let text = output(
            host,
            "moved = b.translate(FreeCAD.Vector(100, 0, 0))\n\
             print(moved.BoundBox.XMin, moved.BoundBox.XMax)\n",
        );
        assert_eq!(text, "100.0 110.0", "{text}");
        let text = output(
            host,
            "turned = b.rotate(FreeCAD.Vector(0, 0, 0), FreeCAD.Vector(0, 0, 1), 90)\n\
             print(round(turned.BoundBox.XMin), round(turned.BoundBox.YMax))\n",
        );
        assert_eq!(text, "-20 10", "{text}");

        // Tessellation: 12 triangles for a box, shared with the display path.
        let text = output(
            host,
            "vertices, facets = b.tessellate(0.1)\n\
             print(len(vertices) >= 8, len(facets))\n",
        );
        assert_eq!(text, "True 12", "{text}");

        // A compound of the two primitives.
        let text = output(
            host,
            "both = Part.makeCompound([b, Part.makeSphere(3)])\n\
             print(both.ShapeType, len(both.Solids))\n",
        );
        assert_eq!(text, "Compound 2", "{text}");
    });
}

#[test]
fn a_script_meshes_a_solid() {
    with_host(|host| {
        let text = output(
            host,
            "b = Part.makeBox(10, 20, 30)\n\
             m = MeshPart.meshFromShape(Shape=b, LinearDeflection=0.5)\n\
             print(m.countFacets(), m.countPoints() >= 8, m.isSolid(), round(m.Area, 1), round(m.Volume, 1))\n",
        );
        assert_eq!(text, "12 True True 2200.0 6000.0", "{text}");

        // The `Mesh(shape)` form is the same mesh.
        let text = output(
            host,
            "direct = Mesh.Mesh(b)\n\
             print(direct.countFacets(), len(direct.Facets) == direct.countFacets())\n",
        );
        assert_eq!(text, "12 True", "{text}");
    });
}

#[test]
fn a_shape_becomes_a_document_object_and_back() {
    let _guard = serial();
    clear_registry();
    std::thread::Builder::new()
        .stack_size(16 << 20)
        .spawn(|| {
            let host = ferrocad_shell_pywasm::live().expect("the interpreter boots");
            ferrocad_core::application()
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .insert_document("Bridge", Document::new());

            // `Part.show` adds a `Part::Feature`; reading it back is the same kernel
            // handle, not a placeholder string.
            let text = output(
                &host,
                "body = Part.show(Part.makeBox(10, 20, 30), 'Body')\n\
                 print(body.Name, body.Shape.ShapeType, body.Shape.Volume)\n",
            );
            assert_eq!(text, "Body Solid 6000.0", "{text}");

            // The typed write the doorway cannot carry: `obj.Shape = shape`.
            let text = output(
                &host,
                "body.Shape = Part.makeSphere(5)\n\
                 print(body.Shape.ShapeType, round(body.Shape.Volume, 1))\n",
            );
            assert_eq!(text, "Solid 523.6", "{text}");

            // A fresh read from the facade sees the document, not a cached instance.
            let text = output(
                &host,
                "print(round(FreeCAD.ActiveDocument.getObject('Body').Shape.Volume, 1))\n",
            );
            assert_eq!(text, "523.6", "{text}");

            // The parametric path: create the object, set its dimensions, recompute.
            let text = output(
                &host,
                "doc = FreeCAD.ActiveDocument\n\
                 param = doc.addObject('Part::Box', 'Param')\n\
                 param.Length = 20\n\
                 doc.recompute()\n\
                 print(param.Shape.ShapeType, round(param.Shape.Volume, 6))\n",
            );
            assert_eq!(text, "Solid 2000.0", "{text}");

            // And the model tree lists both.
            let tree = host.call("model_tree", &[]).expect("model_tree runs");
            let tree: Value = serde_json::from_str(&tree).expect("model_tree is JSON");
            let names: Vec<&str> = tree
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|document| document["objects"].as_array())
                .flatten()
                .filter_map(|object| object["name"].as_str())
                .collect();
            assert!(names.contains(&"Body"), "{names:?}");
            assert!(names.contains(&"Param"), "{names:?}");
        })
        .expect("a worker thread")
        .join()
        .expect("no panic");
}

#[test]
fn a_script_builds_wires_and_faces() {
    let _guard = serial();
    clear_registry();
    with_host(|host| {
        // `makePolygon` uses the same `ferrocad_curves::polygon` the CPython binding
        // does, so the wire has the same three edges and the same closed loop.
        let text = output(
            host,
            "poly = Part.makePolygon(\
                 [FreeCAD.Vector(0, 0, 0), FreeCAD.Vector(10, 0, 0), FreeCAD.Vector(10, 10, 0)],\n\
                 True)\n\
             print(poly.ShapeType, len(poly.Edges), len(poly.Vertexes))\n\
             face = Part.Face(poly)\n\
             print(face.ShapeType, round(face.Area, 3))\n\
             wire = Part.Wire(poly.Edges)\n\
             print(wire.ShapeType, len(wire.Edges))\n",
        );
        assert_eq!(text, "Wire 3 3\nFace 50.0\nWire 3", "{text}");

        // A face with a hole: the second wire is subtracted, not ignored.
        let text = output(
            host,
            "outer = Part.makePolygon(\
                 [FreeCAD.Vector(0, 0, 0), FreeCAD.Vector(10, 0, 0), \
                  FreeCAD.Vector(10, 10, 0), FreeCAD.Vector(0, 10, 0)], True)\n\
             inner = Part.makePolygon(\
                 [FreeCAD.Vector(2, 2, 0), FreeCAD.Vector(4, 2, 0), \
                  FreeCAD.Vector(4, 4, 0), FreeCAD.Vector(2, 4, 0)], True)\n\
             ring = Part.Face([outer, inner])\n\
             print(ring.ShapeType, round(ring.Area, 3), len(ring.Wires))\n",
        );
        assert_eq!(text, "Face 96.0 2", "{text}");
    });
}

/// The same script the browser smoke check runs: a face that came from a wire goes into
/// the document, and painting it is a view-state write the viewport draws.
#[test]
fn a_script_paints_the_face_it_built() {
    let _guard = serial();
    clear_registry();
    std::thread::Builder::new()
        .stack_size(16 << 20)
        .spawn(|| {
            let host = ferrocad_shell_pywasm::live().expect("the interpreter boots");
            ferrocad_core::application()
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .insert_document("Faces", Document::new());

            let text = output(
                &host,
                "import Part, FreeCAD\n\
                 wire = Part.makePolygon([FreeCAD.Vector(0, 0, 0), FreeCAD.Vector(10, 0, 0), \
                                          FreeCAD.Vector(10, 10, 0)], True)\n\
                 panel = Part.show(Part.Face(wire), 'Panel')\n\
                 panel.ViewObject.ShapeAppearance[0].DiffuseColor = (0.2, 0.4, 0.6, 1.0)\n\
                 print(round(panel.Shape.Area, 1), tuple(\
                     round(v, 2) for v in panel.ViewObject.ShapeAppearance[0].DiffuseColor))\n",
            );
            assert_eq!(text, "50.0 (0.2, 0.4, 0.6, 1.0)", "{text}");
        })
        .expect("a worker thread")
        .join()
        .expect("no panic");
}

/// The B-Rep explorer: a face's `Surface` and an edge's `Curve` are the kernel's own
/// classification, the same calls `ferrocad_part_py` makes for the CPython binding.
#[test]
fn a_script_explores_the_surfaces_and_curves_behind_the_brep() {
    with_host(|host| {
        // A box: every face is a plane, and an edge's basis curve is the *infinite*
        // line, as upstream's `TopoShapeEdgePy::getCurve` reports it.
        let text = output(
            host,
            r#"
b = Part.makeBox(10, 20, 30)
s = b.Faces[0].Surface
print(s.TypeId, '|', str(s), '|', repr(s))
print(round(s.Location.x, 3), round(s.Axis.z, 3), hasattr(s, 'Radius'), hasattr(s, 'Center'))
c = b.Edges[0].Curve
print(c.TypeId, '|', str(c), hasattr(c, 'StartPoint'), hasattr(c, 'Center'))
print(round(c.value(5.0).z, 3))
"#,
        );
        assert_eq!(
            text,
            "Part::GeomPlane | <Plane object> | <Plane object>\n\
             0.0 -0.0 False False\n\
             Part::GeomLine | <Line object> False False\n\
             5.0",
            "{text}"
        );

        // The curved kinds carry their frame: a cylinder's radius and axis, a circle
        // edge's centre and closure, a sphere's radius, and a cone's signed semi-angle
        // (negative here because the cone narrows along +Z, as OCCT reports it).
        let text = output(
            host,
            r#"
cyl = Part.makeCylinder(5, 10)
for face in cyl.Faces:
    if face.Surface.TypeId == 'Part::GeomCylinder':
        print('cyl', round(face.Surface.Radius, 3), round(face.Surface.Center.z, 3), round(face.Surface.Axis.z, 3), hasattr(face.Surface, 'Position'))
for edge in cyl.Edges:
    if edge.Curve.TypeId == 'Part::GeomCircle':
        print('edge', round(edge.Curve.Radius, 3), round(edge.Curve.Center.z, 3), round(edge.Curve.Axis.z, 3), edge.Curve.isClosed())
print('sphere', round(Part.makeSphere(5).Faces[0].Surface.Radius, 3))
for face in Part.makeCone(5, 0, 10).Faces:
    if face.Surface.TypeId == 'Part::GeomCone':
        print('cone', round(face.Surface.Radius, 3), round(face.Surface.SemiAngle, 3))
"#,
        );
        assert_eq!(
            text,
            "cyl 5.0 0.0 1.0 False\n\
             edge 5.0 10.0 1.0 True\n\
             edge 5.0 0.0 1.0 True\n\
             sphere 5.0\n\
             cone 5.0 -0.464",
            "{text}"
        );
    });
}

/// `Shape.Placement`: a value assigned to a shape is applied when the shape is read, so
/// the geometry a script measures and the geometry the viewport draws agree. The move
/// stays in the geometry (`translate` keeps returning a moved shape), so the placement a
/// translated shape reports is identity — the same decomposition upstream reports as "the
/// location says where it is", not "the location is the history".
#[test]
fn a_placement_on_a_shape_moves_what_the_shape_reports() {
    with_host(|host| {
        let text = output(
            host,
            r#"
b = Part.makeBox(10, 20, 30)
print(round(b.BoundBox.XMin), round(b.Placement.Base.x))
b.Placement = FreeCAD.Placement(FreeCAD.Vector(100, 0, 0), FreeCAD.Rotation())
print(round(b.BoundBox.XMin), round(b.BoundBox.XMax), round(b.Placement.Base.x))
print(round(b.Volume, 1), len(b.Faces))
moved = b.translate(FreeCAD.Vector(0, 5, 0))
print(round(moved.BoundBox.YMin), round(moved.Placement.Base.y))
"#,
        );
        assert_eq!(text, "0 0\n100 110 100\n6000.0 6\n5 0", "{text}");
    });
}
