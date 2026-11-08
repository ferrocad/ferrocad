//! FerroCAD's shell/engine contract.
//!
//! The GPUI shell ([`ferrocad_shell_core`](https://crates.io/crates/ferrocad_shell_core))
//! is platform-neutral but not engine-neutral: it paints a model tree, a property
//! inspector and a 3D stage, and something has to produce those. This crate owns the
//! **vocabulary** shared by both halves:
//!
//! - [`app::AppBackend`] — the trait a host implements (native: the embedded Python
//!   bridge; browser: a Rust backend over `ferrocad_core`);
//! - [`types`] — the UI-ready view models the trait returns;
//! - the data half of the command, dialog, view, selection and display models.
//!
//! Keeping the contract in its own crate is what makes the physical boundary
//! testable: the shell and any host agree on one definition of every payload, so a
//! backend can be installed and exercised without the shell and the host sharing a
//! crate. It depends only on pure crates — no gpui, no CPython, no GPU.

pub mod app;
pub mod capability;
pub mod catalog;
pub mod chain;
pub mod commands;
pub mod dialog;
pub mod display;
pub mod document_api;
pub mod events;
pub mod runtime;
pub mod selection;
pub mod types;
pub mod view;

pub use app::{AppBackend, NoopListener, ScriptHost};
pub use capability::DocumentGrant;
pub use chain::HostChain;
pub use commands::{CommandInfo, CommandOutcome, Surface, SurfaceGroups, find, group_by_surface};
pub use dialog::{DialogField, DialogSpec, Kind, ValueState};
pub use display::StageMesh;
pub use document_api::DocumentApi;
pub use events::{DocumentEvent, DocumentEventSink};
pub use runtime::{EntryKind, Language, Platform, Runtime};
pub use selection::SelectionItem;
pub use types::{
    AxisSummary, ApplyResult, Bootstrap, DocumentNode, EvalResult, GpuPreferenceOutcome,
    GridProvider, GridSummary, ObjectNode, OpenedDocument, PropRow, ProviderFeature, ProviderSkin,
    SelectionRequest, SetResult, WorkbenchError, WorkbenchReport,
};
pub use view::{ProjectionName, ViewRequest, ViewState, WorkplaneAction};
