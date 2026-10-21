//! Cross-producer conformance (`docs/interaction/navicube-spec.md` §11): the legacy inventor producer
//! and the modern stage producer must render the same cube from the same
//! [`NavicubeVisual`] — same ids, topology, roles, geometry and transforms — and rebuild
//! stably.

use ferrocad_graphics::{Camera, PickId, RenderQueue, Topology, Viewport};
use ferrocad_types::{Matrix4, Rotation};

use super::model::PieceKind;
use super::{
    InventorProducer, NaviCube, NavicubeTheme, NavicubeView, NavicubeVisual, Piece, StageProducer,
};

fn viewport() -> Viewport {
    Viewport::new([640, 480], 1.0)
}

fn theme() -> NavicubeTheme {
    NavicubeTheme::default()
}

fn visual(hilite: Option<Piece>, away: Vec<Piece>) -> NavicubeVisual {
    NavicubeVisual {
        orientation: Rotation::identity(),
        hilite,
        away,
        proximity: false,
        ring: false,
    }
}

/// Rebuild `producer` with the default theme.
fn rebuild(producer: &mut dyn NavicubeView, visual: &NavicubeVisual) {
    producer.rebuild(&viewport(), visual, &theme());
}

/// The colour the spec's role rule assigns a piece's first (pickable) packet.
///
/// Faces and corners lead with their surface fill and bevels with their machined quad;
/// back pieces are dimmed by the producer's depth cue, and a hovered piece still wins over
/// that cue.
fn expected_color(piece: Piece, visual: &NavicubeVisual) -> [f32; 4] {
    let theme = theme();
    let role = piece.role(visual.hilite, &visual.away);
    let back = piece.is_hidden(&visual.orientation.to_matrix());
    match piece.kind() {
        PieceKind::Face => theme.face_fill(
            role,
            &visual.orientation.to_matrix().transform(&piece.direction()),
        ),
        PieceKind::Corner => theme.color(role),
        PieceKind::Edge => theme.bevel_color(role, back, visual.proximity),
    }
}

fn color_of(queue: &RenderQueue, id: PickId) -> Option<[f32; 4]> {
    queue
        .packets
        .iter()
        .find(|packet| packet.state.id == id)
        .map(|packet| packet.state.color)
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-6
}

fn close3(a: [f32; 3], b: [f32; 3]) -> bool {
    close(a[0], b[0]) && close(a[1], b[1]) && close(a[2], b[2])
}

fn close4(a: [f32; 4], b: [f32; 4]) -> bool {
    close(a[0], b[0]) && close(a[1], b[1]) && close(a[2], b[2]) && close(a[3], b[3])
}

fn close16(a: &Matrix4, b: &Matrix4) -> bool {
    a.m.iter().zip(&b.m).all(|(x, y)| (x - y).abs() < 1e-9)
}

/// `(mesh_id, generation, pick_id)` per packet — the renderer's cache key plus identity.
fn keys(queue: &RenderQueue) -> Vec<(u64, u64, u32)> {
    queue
        .packets
        .iter()
        .map(|packet| (packet.mesh_id.0, packet.generation.0, packet.state.id.0))
        .collect()
}

/// Assert two producers' queues draw the same cube.
///
/// The inventor producer adds a separate label packet per visible face (and the stage
/// producer has no texture path), so packet counts differ. Compare the **first packet
/// per pick id** — the glass fill/surface or the edge line — which is the pickable
/// geometry both producers share.
fn compare(a: &RenderQueue, b: &RenderQueue) {
    let ids = |queue: &RenderQueue| {
        let mut ids: Vec<u32> = queue
            .packets
            .iter()
            .map(|packet| packet.state.id.0)
            .filter(|id| (1..=26).contains(id))
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    };
    // Both producers cull the same hidden pieces, so their visible id sets must agree.
    let (ids_a, ids_b) = (ids(a), ids(b));
    assert_eq!(ids_a, ids_b, "visible piece ids");
    for id in ids_a {
        let pa = a
            .packets
            .iter()
            .find(|packet| packet.state.id.0 == id)
            .unwrap_or_else(|| panic!("producer A has no packet for id {id}"));
        let pb = b
            .packets
            .iter()
            .find(|packet| packet.state.id.0 == id)
            .unwrap_or_else(|| panic!("producer B has no packet for id {id}"));
        assert_eq!(pa.state.id, pb.state.id, "pick id");
        assert_eq!(pa.topology, pb.topology, "topology for {:?}", pa.state.id);
        assert!(
            close4(pa.state.color, pb.state.color),
            "colour for {:?}: {:?} vs {:?}",
            pa.state.id,
            pa.state.color,
            pb.state.color
        );
        assert!(
            close16(&pa.state.world_transform, &pb.state.world_transform),
            "transform for {:?}",
            pa.state.id
        );
        // Labels differ (the inventor producer bakes a texture; the stage does not), so
        // compare geometry — positions and normals — not uv.
        let ga = pa.geometry.as_ref().expect("inventor geometry");
        let gb = pb.geometry.as_ref().expect("stage geometry");
        assert_eq!(ga.indices, gb.indices, "indices for {:?}", pa.state.id);
        assert_eq!(
            ga.vertices.len(),
            gb.vertices.len(),
            "vertex count for {:?}",
            pa.state.id
        );
        for (va, vb) in ga.vertices.iter().zip(&gb.vertices) {
            assert!(
                close3(va.position, vb.position),
                "position for {:?}",
                pa.state.id
            );
            assert!(close3(va.normal, vb.normal), "normal for {:?}", pa.state.id);
        }
    }
}

#[test]
fn both_producers_emit_the_same_cube() {
    let visual = visual(Some(Piece::TOP), vec![Piece::BOTTOM]);
    let mut inventor = InventorProducer::new();
    let mut stage = StageProducer::new();
    rebuild(&mut inventor, &visual);
    rebuild(&mut stage, &visual);
    compare(inventor.queue(), stage.queue());
}

#[test]
fn both_producers_assign_the_documented_roles() {
    let away = vec![Piece::BOTTOM];
    let hilite = Some(Piece::TOP);
    let visual = visual(hilite, away.clone());

    let mut inventor = InventorProducer::new();
    let mut stage = StageProducer::new();
    for producer in [
        &mut inventor as &mut dyn NavicubeView,
        &mut stage as &mut dyn NavicubeView,
    ] {
        rebuild(producer, &visual);
        let queue = producer.queue();
        let rotation = visual.orientation.to_matrix();
        for piece in Piece::ALL
            .iter()
            .copied()
            .filter(|piece| !piece.is_hidden(&rotation))
        {
            let color = color_of(queue, piece.pick_id()).expect("a packet per visible piece");
            assert!(
                close4(color, expected_color(piece, &visual)),
                "{piece:?} has the wrong role colour"
            );
        }
        // Back-facing pieces are culled, not merely dimmed.
        for piece in Piece::ALL
            .iter()
            .copied()
            .filter(|piece| piece.is_hidden(&rotation))
        {
            assert!(
                color_of(queue, piece.pick_id()).is_none(),
                "{piece:?} is hidden but emitted"
            );
        }
    }
}

#[test]
fn a_custom_theme_recolours_both_producers() {
    let theme = NavicubeTheme {
        base: [0.1, 0.2, 0.3, 1.0],
        highlighted: [0.9, 0.8, 0.7, 1.0],
        neighbour: [0.9, 0.7, 0.5, 0.4],
        away: [0.4, 0.4, 0.4, 0.5],
        bevel: [0.8, 0.9, 1.0, 0.5],
        bevel_proximity: [0.85, 0.92, 1.0, 0.7],
        bevel_away: [0.3, 0.3, 0.3, 0.2],
        edge: [0.5, 0.5, 0.5, 0.4],
        edge_proximity: [0.7, 0.7, 0.7, 0.8],
        edge_highlighted: [0.1, 0.8, 1.0, 1.0],
        edge_neighbour: [0.2, 0.7, 0.9, 0.8],
        edge_away: [0.3, 0.3, 0.3, 0.2],
        label: [0.6, 0.6, 0.6, 0.5],
        label_proximity: [0.8, 0.8, 0.8, 0.9],
        label_highlighted: [1.0, 1.0, 1.0, 1.0],
        glow: [0.1, 0.8, 1.0, 0.3],
        ring: [0.5, 0.5, 0.5, 0.4],
        ring_proximity: [0.7, 0.7, 0.7, 0.8],
        ring_highlighted: [0.1, 0.8, 1.0, 1.0],
        turntable: [0.1, 0.1, 0.1, 0.6],
        ring_fill: [0.6, 0.7, 0.8, 0.4],
        ring_fill_proximity: [0.7, 0.8, 0.9, 0.5],
        ring_fill_highlighted: [0.1, 0.8, 1.0, 0.6],
        edge_width: 2.0,
    };
    let visual = visual(Some(Piece::TOP), vec![Piece::BOTTOM]);
    let mut inventor = InventorProducer::new();
    let mut stage = StageProducer::new();
    inventor.rebuild(&viewport(), &visual, &theme);
    stage.rebuild(&viewport(), &visual, &theme);
    for producer in [&inventor as &dyn NavicubeView, &stage as &dyn NavicubeView] {
        let queue = producer.queue();
        assert!(close4(
            color_of(queue, Piece::TOP.pick_id()).unwrap(),
            expected_color(Piece::TOP, &visual)
        ));
        assert!(
            color_of(queue, Piece::BOTTOM.pick_id()).is_none(),
            "back-facing faces are culled"
        );
        assert!(close4(
            color_of(queue, Piece::FRONT.pick_id()).unwrap(),
            expected_color(Piece::FRONT, &visual)
        ));
    }
}

#[test]
fn rebuild_is_stable_within_each_producer() {
    let visual = visual(Some(Piece::TOP), vec![Piece::BOTTOM]);
    let mut inventor = InventorProducer::new();
    let mut stage = StageProducer::new();
    for producer in [
        &mut inventor as &mut dyn NavicubeView,
        &mut stage as &mut dyn NavicubeView,
    ] {
        rebuild(producer, &visual);
        let first = keys(producer.queue());
        rebuild(producer, &visual);
        let second = keys(producer.queue());
        assert_eq!(first, second, "mesh ids and generations must be stable");
    }
}

#[test]
fn the_stage_cube_hosts_and_picks() {
    let mut cube = NaviCube::stage(viewport());
    cube.sync(&Camera::identity());
    let rect = cube.rect();
    let centre = [
        rect.x + rect.width as i32 / 2,
        rect.y + rect.height as i32 / 2,
    ];
    assert_eq!(cube.hit(centre), Some(Piece::TOP));
}

#[test]
fn both_producers_draw_the_filled_glyphs() {
    let theme = theme();
    let mut visual = visual(Some(Piece::TOP), vec![Piece::BOTTOM]);
    visual.proximity = true;

    let mut inventor = InventorProducer::new();
    let mut stage = StageProducer::new();
    rebuild(&mut inventor, &visual);
    rebuild(&mut stage, &visual);

    for producer in [&inventor as &dyn NavicubeView, &stage as &dyn NavicubeView] {
        let queue = producer.queue();

        // The front and right faces carry filled glyph packets; the away back face does not.
        for piece in [Piece::FRONT, Piece::RIGHT] {
            let role = piece.role(visual.hilite, &visual.away);
            assert!(
                queue.packets.iter().any(|packet| {
                    packet.state.id == piece.pick_id()
                        && packet.topology == Topology::Triangles
                        && close4(
                            packet.state.color,
                            theme.label_color(role, visual.proximity),
                        )
                }),
                "{piece:?} should carry a filled glyph"
            );
        }
        assert!(
            !queue.packets.iter().any(|packet| {
                packet.state.id == Piece::BOTTOM.pick_id()
                    && packet.topology == Topology::Triangles
                    && close4(
                        packet.state.color,
                        theme.label_color(
                            Piece::BOTTOM.role(visual.hilite, &visual.away),
                            visual.proximity,
                        ),
                    )
            }),
            "an away face must not draw its glyph"
        );
    }
}

#[test]
fn a_config_repositions_the_cube() {
    use super::{Corner, NaviCubeConfig};
    let cube = NaviCube::new(viewport()).with_config(NaviCubeConfig {
        corner: Corner::BottomLeft,
        cube_size: 48.0,
        margin: 4.0,
    });
    let rect = cube.rect();
    // The overlay box is derived from the cube size: round(48 · 2·1.28/√3) = 71.
    assert_eq!(rect.width, 71);
    // The box is centred in the `CHROME_WIDTH` column: 4 + (248 − 71)/2.
    assert_eq!(rect.x, 4 + (248 - 71) / 2);
    assert_eq!(rect.y, 4);
}
