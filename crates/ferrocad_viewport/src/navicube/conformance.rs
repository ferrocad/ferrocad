//! Cross-producer conformance (`docs/navicube-spec.md` §11): the legacy inventor producer
//! and the modern stage producer must render the same cube from the same
//! [`NavicubeVisual`] — same ids, topology, roles, geometry and transforms — and rebuild
//! stably.

use ferrocad_graphics::{Camera, PickId, RenderQueue, Viewport};
use ferrocad_types::{Matrix4, Rotation};

use super::model::HOME_PICK_ID;
use super::theme::{BASE_COLOR, HILITE_COLOR, HOME_COLOR, INACTIVE_COLOR};
use super::{
    InventorProducer, NaviCube, NaviTarget, NavicubeTheme, NavicubeView, NavicubeVisual, Piece,
    StageProducer,
};

fn viewport() -> Viewport {
    Viewport::new([640, 480], 1.0)
}

fn theme() -> NavicubeTheme {
    NavicubeTheme::default()
}

fn visual(hilite: Option<NaviTarget>, away: Vec<Piece>) -> NavicubeVisual {
    NavicubeVisual {
        orientation: Rotation::identity(),
        hilite,
        away,
    }
}

/// Rebuild `producer` with the default theme.
fn rebuild(producer: &mut dyn NavicubeView, visual: &NavicubeVisual) {
    producer.rebuild(&viewport(), visual, &theme());
}

/// The colour the spec's role rule assigns a piece.
fn expected_color(piece: Piece, hilite: Option<NaviTarget>, away: &[Piece]) -> [f32; 4] {
    if hilite == Some(NaviTarget::Piece(piece)) {
        HILITE_COLOR
    } else if away.contains(&piece) {
        INACTIVE_COLOR
    } else {
        BASE_COLOR
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

/// Assert two producers' queues draw the same cube, packet for packet.
fn compare(a: &RenderQueue, b: &RenderQueue) {
    assert_eq!(a.len(), 27, "26 pieces plus the Home button");
    assert_eq!(a.len(), b.len(), "packet count");
    for (pa, pb) in a.packets.iter().zip(&b.packets) {
        assert_eq!(pa.state.id, pb.state.id, "draw order / pick id");
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
            assert!(close3(va.position, vb.position), "position for {:?}", pa.state.id);
            assert!(close3(va.normal, vb.normal), "normal for {:?}", pa.state.id);
        }
    }
}

#[test]
fn both_producers_emit_the_same_cube() {
    let visual = visual(Some(NaviTarget::Piece(Piece::TOP)), vec![Piece::BACK]);
    let mut inventor = InventorProducer::new();
    let mut stage = StageProducer::new();
    rebuild(&mut inventor, &visual);
    rebuild(&mut stage, &visual);
    compare(inventor.queue(), stage.queue());
}

#[test]
fn both_producers_assign_the_documented_roles() {
    let away = vec![Piece::BACK];
    let hilite = Some(NaviTarget::Piece(Piece::TOP));
    let visual = visual(hilite, away.clone());

    let mut inventor = InventorProducer::new();
    let mut stage = StageProducer::new();
    for producer in [
        &mut inventor as &mut dyn NavicubeView,
        &mut stage as &mut dyn NavicubeView,
    ] {
        rebuild(producer, &visual);
        let queue = producer.queue();
        for piece in Piece::ALL {
            let color = color_of(queue, piece.pick_id()).expect("a packet per piece");
            assert!(
                close4(color, expected_color(piece, hilite, &away)),
                "{piece:?} has the wrong role colour"
            );
        }
        let home = color_of(queue, HOME_PICK_ID).expect("the Home packet");
        assert!(close4(home, HOME_COLOR), "the Home button is not highlighted");
    }
}

#[test]
fn a_custom_theme_recolours_both_producers() {
    let theme = NavicubeTheme {
        base: [0.1, 0.2, 0.3, 1.0],
        highlighted: [0.9, 0.8, 0.7, 1.0],
        away: [0.4, 0.4, 0.4, 0.5],
        home: [0.2, 0.4, 0.6, 1.0],
        edge_width: 2.0,
    };
    let visual = visual(Some(NaviTarget::Piece(Piece::TOP)), vec![Piece::BACK]);
    let mut inventor = InventorProducer::new();
    let mut stage = StageProducer::new();
    inventor.rebuild(&viewport(), &visual, &theme);
    stage.rebuild(&viewport(), &visual, &theme);
    for producer in [
        &inventor as &dyn NavicubeView,
        &stage as &dyn NavicubeView,
    ] {
        let queue = producer.queue();
        assert!(close4(
            color_of(queue, Piece::TOP.pick_id()).unwrap(),
            theme.highlighted
        ));
        assert!(close4(color_of(queue, Piece::BACK.pick_id()).unwrap(), theme.away));
        assert!(close4(color_of(queue, Piece::FRONT.pick_id()).unwrap(), theme.base));
        assert!(close4(color_of(queue, HOME_PICK_ID).unwrap(), theme.home));
    }
}

#[test]
fn both_producers_highlight_the_home_button() {
    let visual = visual(Some(NaviTarget::Home), vec![]);
    let mut inventor = InventorProducer::new();
    let mut stage = StageProducer::new();
    for producer in [
        &mut inventor as &mut dyn NavicubeView,
        &mut stage as &mut dyn NavicubeView,
    ] {
        rebuild(producer, &visual);
        let home = color_of(producer.queue(), HOME_PICK_ID).expect("the Home packet");
        assert!(close4(home, HILITE_COLOR), "the Home button should be highlighted");
    }
}

#[test]
fn rebuild_is_stable_within_each_producer() {
    let visual = visual(Some(NaviTarget::Piece(Piece::TOP)), vec![Piece::BACK]);
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
    assert_eq!(cube.hit(centre), Some(NaviTarget::Piece(Piece::FRONT)));
}

#[test]
fn a_config_repositions_the_cube() {
    use super::{Corner, NaviCubeConfig};
    let cube = NaviCube::new(viewport()).with_config(NaviCubeConfig {
        corner: Corner::BottomLeft,
        size: 48.0,
        margin: 4.0,
    });
    let rect = cube.rect();
    assert_eq!(rect.width, 48);
    assert_eq!(rect.x, 4);
    assert_eq!(rect.y, 4);
}
