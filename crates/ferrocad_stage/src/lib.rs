//! # FerroCAD Stage (`ferrocad_stage`)
//!
//! The **modern** presentation side of the FerroCAD visuals: a declarative CAD
//! framework that compiles *straight* to the shared draw-batch vocabulary, replacing
//! the imperative `pivy.coin` node tree for new workbenches.
//!
//! A workbench declares high-level visual items (a [`ShapeItem`] with tessellation and
//! display attributes) on a [`Stage`]; [`Stage::emit_batches`] decomposes each into
//! [`DrawPacket`](ferrocad_inventor::DrawPacket)s. There is **no intermediate scene
//! graph** — no node arena, no traversal — so a stage is cheap to rebuild each frame.
//!
//! ```text
//! Stage ──emit_batches──▶ RenderQueue { DrawPacket … } ──DrawBatch──▶ renderer
//!                        ▲
//!       ferrocad_inventor::compile ──┘   (the legacy pivy.coin path emits the same)
//! ```
//!
//! Because both stacks emit the **same** batch vocabulary, a renderer draws them in one
//! pass against one depth buffer — no visual layering artifacts (see the module docs of
//! [`ferrocad_inventor`](ferrocad_inventor)).
//!
//! This crate is pure Rust: no `pyo3`, no `wgpu`. The Python surface (`ferrocad.stage`)
//! and the interactive tools/gizmos are later slices; see `docs/stage.md`.
//!
//! # Example
//!
//! ```
//! use ferrocad_stage::{DrawMode, ShapeItem, Stage};
//!
//! // A triangle with its three edges, shaded with edges.
//! let item = ShapeItem::new(7)
//!     .with_draw_mode(DrawMode::ShadedWithEdges)
//!     .with_shape(
//!         vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
//!         vec![],
//!         vec![0, 1, 2],
//!         vec![0, 1, 1, 2, 2, 0],
//!     );
//!
//! let queue = Stage::new().with_item(item).to_queue();
//! // One shaded surface packet and one wireframe packet.
//! assert_eq!(queue.len(), 2);
//! ```

pub mod shape;
pub mod stage;

pub use shape::{DrawMode, ShapeItem};
pub use stage::Stage;
