//! The shell's view models: the UI-ready payloads an [`AppBackend`](crate::app::AppBackend)
//! returns.
//!
//! They are deliberately plain data with `serde` derives. A backend that speaks
//! JSON (the native Python bridge) decodes straight into them; a backend written in
//! Rust (the browser host) constructs them directly. The shell never learns which.

use serde::Deserialize;

/// Shell status returned by [`AppBackend::bootstrap`](crate::app::AppBackend::bootstrap).
#[derive(Debug, Clone, Deserialize)]
pub struct Bootstrap {
    pub version: String,
    pub backend: String,
    pub document: String,
}

/// The document `open_document` loaded.
#[derive(Debug, Clone, Deserialize)]
pub struct OpenedDocument {
    pub document: String,
    #[serde(default)]
    pub objects: usize,
}

/// The outcome of loading workbench scripts: what loaded, and what raised.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct WorkbenchReport {
    #[serde(default)]
    pub loaded: Vec<String>,
    #[serde(default)]
    pub errors: Vec<WorkbenchError>,
}

/// A workbench script that raised. Loading is best-effort: the shell reports
/// these instead of failing.
#[derive(Debug, Clone, Deserialize)]
pub struct WorkbenchError {
    pub workbench: String,
    pub script: String,
    #[serde(default)]
    pub traceback: String,
}

/// A document in the model tree.
#[derive(Debug, Clone, Deserialize)]
pub struct DocumentNode {
    pub name: String,
    pub label: String,
    pub objects: Vec<ObjectNode>,
}

/// An object inside a document.
#[derive(Debug, Clone, Deserialize)]
pub struct ObjectNode {
    pub name: String,
    pub label: String,
    #[serde(rename = "type")]
    pub type_id: String,
}

/// One row of the property editor.
#[derive(Debug, Clone, Deserialize)]
pub struct PropRow {
    pub name: String,
    /// A display label for a generated row (a command button). Empty for a normal
    /// property, whose label is its name.
    #[serde(default)]
    pub label: String,
    #[serde(rename = "type")]
    pub type_id: String,
    pub value: String,
    pub status: String,
    /// Whether the editor can change this property.
    #[serde(default)]
    pub editable: bool,
    /// The command this row runs instead of editing a value (`Reset to edition
    /// defaults` on a settings card).
    #[serde(default)]
    pub command: Option<String>,
    /// The editor kind (`text`, `bool`, `int`, `float`, `quantity`, `enum`,
    /// `color`). It picks the control: a `color` row gets a swatch and a palette,
    /// a bounded number gets the slider overlay.
    #[serde(default)]
    pub kind: String,
    /// The editor's inclusive bounds, when the schema declares them.
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    /// The quantity unit (`mm`), shown in the slider's readout.
    #[serde(default)]
    pub unit: Option<String>,
    /// An enumeration's options. Empty for every other kind; an enum row with
    /// choices renders as a select box instead of a text field.
    #[serde(default)]
    pub choices: Vec<String>,
}

/// The outcome of a property edit.
#[derive(Debug, Clone, Deserialize)]
pub struct SetResult {
    pub ok: bool,
    #[serde(default)]
    pub error: String,
    /// A presentation change the edit asks for (a settings row that changes the grid
    /// spacing or the gizmo palette). Without this the edit would be stored but the
    /// viewport would keep drawing the old state until something else repainted it.
    #[serde(default)]
    pub view: Option<crate::view::ViewRequest>,
}

impl Default for SetResult {
    fn default() -> Self {
        Self {
            ok: false,
            error: String::new(),
            view: None,
        }
    }
}

/// The outcome of evaluating a console line.
#[derive(Debug, Clone, Deserialize)]
pub struct EvalResult {
    pub ok: bool,
    pub output: String,
}

/// The outcome of a dress-up edit.
#[derive(Debug, Clone, Deserialize)]
pub struct ApplyResult {
    pub ok: bool,
    #[serde(default)]
    pub error: String,
}

impl Default for ApplyResult {
    fn default() -> Self {
        Self {
            ok: false,
            error: String::new(),
        }
    }
}

/// The grid's active provider and spacing, for the header popover (T4).
///
/// The shape is closed: with no provider registered, `backend`/`label`/`spacing`
/// are `None` and `providers` is empty, so the popover never special-cases it.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct GridSummary {
    #[serde(default)]
    pub feature: String,
    #[serde(default)]
    pub backend: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    /// The active provider's spacing as its display string (`"10 mm"`).
    #[serde(default)]
    pub spacing: Option<String>,
    #[serde(default)]
    pub providers: Vec<GridProvider>,
}

/// One registered grid backend, as the popover lists it.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct GridProvider {
    #[serde(default)]
    pub backend: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub active: bool,
    /// The registry id this backend owns (`None` when it offers no command).
    ///
    /// The Dev Tools command list uses it to disable the losing backend's entry,
    /// so two same-purpose commands do not look equally runnable. This is a UI
    /// filter only: the registry is advisory and cannot stop a mod from drawing.
    #[serde(default)]
    pub command: Option<String>,
}

/// The axis cross's active provider and geometry, for the header popover.
///
/// The shape mirrors [`GridSummary`]: with no axis provider registered,
/// `backend`/`label`/`length`/`width` are `None` and `providers` is empty, so the
/// popover never special-cases the empty state. The providers reuse
/// [`GridProvider`] (backend, label, active) — the list has the same meaning.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct AxisSummary {
    #[serde(default)]
    pub feature: String,
    #[serde(default)]
    pub backend: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    /// The stem length as its display string (`"80 px"`).
    #[serde(default)]
    pub length: Option<String>,
    /// The stem width as its display string (`"1.5 px"`).
    #[serde(default)]
    pub width: Option<String>,
    #[serde(default)]
    pub providers: Vec<GridProvider>,
}

/// One provider feature and its backends, for the Dev Tools providers panel.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct ProviderFeature {
    #[serde(default)]
    pub feature: String,
    #[serde(default)]
    pub providers: Vec<GridProvider>,
}

/// The result of persisting a GPU adapter preference.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct GpuPreferenceOutcome {
    /// Whether the value was accepted and stored.
    pub ok: bool,
    /// The stored value (empty when rejected).
    #[serde(default)]
    pub value: String,
    /// Why the value was rejected; empty on success.
    #[serde(default)]
    pub error: String,
}

/// One write a script queued on `FreeCADGui.Selection`.
///
/// `op` is `add`, `remove`, `clear` or `preselect`. `object` is a document object
/// name (empty for `clear`), `elements` the kernel sub-element names, and `points`
/// the world points the pick reported (as far as the script knew them). The shape is
/// deliberately the facade's own record so the two sides cannot drift.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct SelectionRequest {
    pub op: String,
    #[serde(default)]
    pub object: String,
    #[serde(default)]
    pub elements: Vec<String>,
    #[serde(default)]
    pub points: Vec<(f64, f64, f64)>,
}

/// The WGSL skin the active provider of a feature supplies, if any.
///
/// The shader lives in the mod, not in the app: the registry is the only channel.
/// It answers for the **active** backend, so a provider switch can never leave a
/// stale skin behind in the shell.
#[derive(Debug, Clone, Deserialize)]
pub struct ProviderSkin {
    /// The source text, or `None` when the active backend supplies no skin.
    pub source: Option<String>,
}
