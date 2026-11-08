//! # ferrocad
//!
//! The base FerroCAD application. The binary (`cargo install ferrocad`) boots an
//! embedded CPython interpreter and opens the `bite-gpui` shell; the engine is
//! [`ferrocad_core`](https://crates.io/crates/ferrocad_core) and the shell library
//! is [`ferrocad_shell_python`](https://crates.io/crates/ferrocad_shell_python).
//!
//! ## The payload
//!
//! The app is useless without its Python facade and workbenches. A development
//! checkout has them in the workspace; `cargo install` does not, so the payload
//! is embedded into the binary and unpacked on first run. See [`payload`] for the
//! resolution order and the packaging story in `docs/developer/architecture/distribution.md`.
//!
//! ## Editions
//!
//! [`run_as`] is the entry point an **edition** (a thin binary that links extra
//! workbench modules and brands the window) calls. An edition registers its own
//! built-in modules first, then delegates here; see `docs/developer/architecture/repackaging.md`.

use ferrocad_shell_api::Runtime;
use ferrocad_shell_python::HostConfig;

pub mod payload;

/// The scripting configurations a host can install (`--script-host=`).
pub use ferrocad_shell_python::Runtime as ScriptHost;

/// The workbench contributions an edition registers (its manipulators).
pub use ferrocad_shell_python::Contributions;

/// Register the core `ferrocad` PyO3 extension as a built-in module, so `import
/// ferrocad` resolves without a separate `ferrocad.abi3.so` on disk. `#[pymodule]`
/// expands `ferrocad_py::ferrocad` into a hidden module, which the macro needs in
/// scope by name.
///
/// Must run before the interpreter initialises.
pub fn register_core_module() {
    use ferrocad_py::ferrocad as ferrocad_module;
    pyo3::append_to_inittab!(ferrocad_module);
}

/// Boot the embedded interpreter and open the shell window (the base app).
pub fn run() {
    run_as("FerroCAD");
}

/// Like [`run`], but with an edition's `app_name` (the window title) and its Python
/// entry module.
///
/// An edition calls this after registering its own built-in modules, so they share
/// the process-wide core the shell boots (see `docs/developer/architecture/occt-integration.md` §4). The
/// entry module is the edition's **app script**: it re-exports the base shell's data
/// functions and registers its workbenches' module hooks (see
/// `docs/developer/architecture.md` §9).
pub fn run_as(app_name: &str) {
    run_edition(app_name, "ferrocad_shell");
}

/// [`run_as`] with an explicit entry module (the edition's app script).
pub fn run_edition(app_name: &str, entry_module: &str) {
    run_edition_with(app_name, entry_module, base_contributions());
}

/// The general edition's contributions: the generic transform handle and the native
/// workplane pick.
///
/// A whole-object selection arms a translate gizmo, so the shipped app has an
/// interactive move tool out of the box (the wheel's `Move`, or `Ctrl+Shift+M`, arms it
/// for the current selection). The `workplane_points` tool is what `App_Workplane3Points`
/// starts: the
/// command names a registry id, so the tool ships with the app rather than with a
/// workbench. Workbench manipulators — the Part edge dress-up arc, a height grip —
/// stay with the edition that links them, which is why this is a default and not a
/// global registration in the shell.
///
/// Editions start from this rather than from `Contributions::default()`, so adding a
/// workbench manipulator cannot silently drop the shared move gizmo:
///
/// ```ignore
/// let contributions = ferrocad::base_contributions()
///     .with_manipulator(ferrocad_part_gui::fillet_manipulator);
/// ```
pub fn base_contributions() -> Contributions {
    Contributions::new()
        .with_gizmo(ferrocad_shell_python::translate_gizmo)
        .with_tool("workplane_points", || {
            Box::new(ferrocad_stage::PlanePointsTool::new()) as Box<dyn ferrocad_stage::Tool>
        })
}

/// [`run_edition`] with the workbench contributions the edition registers.
///
/// An edition that links a workbench with a manipulator (Part's edge dress-up) passes
/// its factories here, so the shell can host them without naming the workbench. An
/// edition with no manipulators passes [`Contributions::default`].
pub fn run_edition_with(
    app_name: &str,
    entry_module: &str,
    contributions: Contributions,
) {
    run_edition_with_document(app_name, entry_module, contributions, None);
}

/// [`run_edition_with`] with a document to open at boot (`--open`).
///
/// The file is opened after the app script's bootstrap, then the sample document
/// is dropped. The window's title, model tree, properties and viewport all follow
/// the opened document, which is the basic `.FCStd` viewer path.
pub fn run_edition_with_document(
    app_name: &str,
    entry_module: &str,
    contributions: Contributions,
    open: Option<std::path::PathBuf>,
) {
    run_edition_with_script(app_name, entry_module, contributions, open, Vec::new(), Vec::new());
}

/// [`run_edition_with_document`] with the scripting configuration `--script-host=` asked
/// for.
///
/// The window is the same in every configuration; what changes is the half behind it —
/// the edition's CPython by default, the document alone, or (when the build linked them)
/// the browser's RustPython and JS pages. More than one entry installs a chain whose first
/// half is the primary (B8c). A configuration this build cannot serve is reported and
/// replaced by the document half.
pub fn run_edition_with_script(
    app_name: &str,
    entry_module: &str,
    contributions: Contributions,
    open: Option<std::path::PathBuf>,
    script_hosts: Vec<Runtime>,
    addons: Vec<std::path::PathBuf>,
) {
    register_core_module();
    let mut config = app_config(app_name, entry_module);
    config.contributions = contributions;
    config.open_document = open;
    config.script_hosts = script_hosts;
    config.addons = addons;
    ferrocad_shell_python::run_with(config);
}

/// Boot the interpreter and the payload, then exit without opening a window.
///
/// This backs the app's `--check-init` flag: it loads the native dependencies,
/// the built-in `ferrocad` module, the `FreeCAD` facade and the workbenches
/// exactly as a normal start does, and reports success. It needs no display or
/// GPU, so the release workflow runs it on an unpacked artifact to catch a broken
/// layout before shipping (see `docs/developer/architecture/distribution.md`).
pub fn check_init() -> Result<(), String> {
    register_core_module();
    ferrocad_shell_python::check_init(base_config("FerroCAD"))
}

pub use ferrocad_shell_python::RenderReport;

/// Render an edition's sample document headlessly to `out`.
///
/// This is the `--render <png>` path: the same boot as `--check-init`, then the
/// edition's display hook supplies the tessellated document shapes, which are
/// compiled to a stage and drawn with one offscreen wgpu pass. It needs a wgpu
/// adapter (a software Vulkan ICD is enough) but no display server. The edition's
/// own built-in modules must already be registered — `ferrocad_parametric` does
/// that for `Part` before it calls this.
pub fn render_edition_headless(
    app_name: &str,
    entry_module: &str,
    render: Option<&std::path::Path>,
    stl: Option<&std::path::Path>,
    size: [u32; 2],
    script: Option<&std::path::Path>,
    open: Option<&std::path::Path>,
) -> Result<RenderReport, String> {
    register_core_module();
    let mut config = app_config(app_name, entry_module);
    config.open_document = open.map(std::path::Path::to_path_buf);
    ferrocad_shell_python::headless::run(&config, render, stl, size, script)
}

/// The flags an edition accepts in both its windowed and headless modes.
#[derive(Debug, Clone)]
pub struct EditionArgs {
    /// `--render <png>`: render headlessly.
    pub render: Option<std::path::PathBuf>,
    /// `--stl <path>`: export the tessellated stage as binary STL.
    pub stl: Option<std::path::PathBuf>,
    /// The framebuffer size for `--render`.
    pub size: [u32; 2],
    /// `--script <py>`: build the model before rendering/exporting.
    pub script: Option<std::path::PathBuf>,
    /// `--open <file>`: open a saved document at boot (windowed or headless).
    pub open: Option<std::path::PathBuf>,
    /// `--script-host <names>`: which scripting halves the window installs (`plain`,
    /// `cpython`, `pywasm`, `js`), comma-separated for more than one. Empty leaves it to
    /// `FERROCAD_SCRIPT_HOST`, then to CPython. The first name is the primary.
    pub script_hosts: Vec<Runtime>,
    /// `--addons <path>`: a wasm workbench bundle or a directory of them
    /// (`<dir>/<Name>/addon.wasm`), repeatable. When empty, the bundles under the mods
    /// directories are loaded instead (B9b), so the flag is only needed to point at a
    /// bundle somewhere else.
    ///
    /// Addons ride beside the script half rather than replacing it, which is how the
    /// browser loads them (`?addons=`). Each bundle is sandboxed: its only capability is
    /// the document doorway.
    pub addons: Vec<std::path::PathBuf>,
}

impl Default for EditionArgs {
    fn default() -> Self {
        Self {
            render: None,
            stl: None,
            size: [800, 600],
            script: None,
            open: None,
            script_hosts: Vec::new(),
            addons: Vec::new(),
        }
    }
}

/// Parse `--render`, `--size`, `--script` and `--open` from the process arguments.
///
/// Unknown arguments are left to the caller's normal handling; this parser only
/// consumes the flags it owns.
pub fn edition_args() -> Result<EditionArgs, String> {
    let mut parsed = EditionArgs::default();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--render" => {
                let path = args
                    .next()
                    .ok_or_else(|| "--render wants a path".to_string())?;
                parsed.render = Some(std::path::PathBuf::from(path));
            }
            "--stl" => {
                let path = args
                    .next()
                    .ok_or_else(|| "--stl wants a path".to_string())?;
                parsed.stl = Some(std::path::PathBuf::from(path));
            }
            "--script" => {
                let path = args
                    .next()
                    .ok_or_else(|| "--script wants a path".to_string())?;
                parsed.script = Some(std::path::PathBuf::from(path));
            }
            "--open" => {
                let path = args
                    .next()
                    .ok_or_else(|| "--open wants a path".to_string())?;
                parsed.open = Some(std::path::PathBuf::from(path));
            }
            "--script-host" => {
                let names = args.next().ok_or_else(|| {
                    "--script-host wants plain, cpython, pywasm or js".to_string()
                })?;
                parsed.script_hosts.extend(Runtime::parse_list(&names)?);
            }
            flag if flag.starts_with("--script-host=") => {
                parsed
                    .script_hosts
                    .extend(Runtime::parse_list(&flag["--script-host=".len()..])?);
            }
            "--addons" => {
                let path = args
                    .next()
                    .ok_or_else(|| "--addons wants a bundle or a directory".to_string())?;
                parsed.addons.push(std::path::PathBuf::from(path));
            }
            flag if flag.starts_with("--addons=") => {
                parsed
                    .addons
                    .push(std::path::PathBuf::from(&flag["--addons=".len()..]));
            }
            "--size" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--size wants WIDTHxHEIGHT".to_string())?;
                parsed.size = parse_size(&value)?;
            }
            _ => {}
        }
    }
    // The flag is repeatable, so the one-host-per-language rule is checked once the whole
    // command line has been read rather than per occurrence.
    Runtime::validate_list(&parsed.script_hosts)?;
    Ok(parsed)
}

/// `800x600` → `[800, 600]`.
fn parse_size(value: &str) -> Result<[u32; 2], String> {
    let (width, height) = value
        .split_once('x')
        .ok_or_else(|| format!("--size wants WIDTHxHEIGHT, got {value:?}"))?;
    let parse = |part: &str| {
        part.parse::<u32>()
            .ok()
            .filter(|n| *n > 0)
            .ok_or_else(|| format!("--size wants positive integers, got {value:?}"))
    };
    Ok([parse(width)?, parse(height)?])
}

/// The [`HostConfig`] an edition boots with, after resolving its payload.
fn base_config(app_name: &str) -> HostConfig {
    app_config(app_name, "ferrocad_shell")
}

fn app_config(app_name: &str, entry_module: &str) -> HostConfig {
    let payload = payload::resolve();
    HostConfig {
        app_name: app_name.to_string(),
        entry_module: entry_module.to_string(),
        python_paths: payload.python_paths,
        mods_paths: payload.mods_paths,
        contributions: Contributions::default(),
        open_document: None,
        script_hosts: Vec::new(),
        addons: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use ferrocad_geom::SelectionRef;
    use ferrocad_stage::{ShapeItem, Stage};

    use super::*;

    fn stage_with_a_box() -> Stage {
        let mut stage = Stage::new();
        stage.add_item(ShapeItem::new(7).with_shape(
            vec![[0.0, 0.0, 0.0], [2.0, 2.0, 4.0]],
            Vec::new(),
            Vec::new(),
            Vec::new(),
        ));
        stage
    }

    /// An edition adds its workbench manipulators on top of [`base_contributions`],
    /// so the shared move gizmo must survive the composition.
    ///
    /// The Part edition used to start from `Contributions::default()`, which quietly
    /// left a selected body with no triad while the toy edition (which does start from
    /// the base) still had one.
    #[test]
    fn adding_a_manipulator_keeps_the_shared_move_gizmo() {
        let stage = stage_with_a_box();
        let selection = SelectionRef::whole(7);

        let base = base_contributions();
        assert_eq!(
            base.gizmo_for(&selection, &stage)
                .map(|gizmo| gizmo.target_id()),
            Some(7),
            "the base composition arms the move gizmo for a whole-object selection"
        );

        let with_manipulator =
            base_contributions().with_manipulator(|_: &SelectionRef, _: &Stage| None);
        assert!(
            with_manipulator.gizmo_for(&selection, &stage).is_some(),
            "a workbench manipulator must not replace the gizmo registry"
        );

        // The shape that dropped the triad, kept as the contrast for the assertion above.
        let without_base =
            Contributions::default().with_manipulator(|_: &SelectionRef, _: &Stage| None);
        assert!(without_base.gizmo_for(&selection, &stage).is_none());
    }

    /// A Part click resolves to a *named* element (`Face1`, `Edge1`, …), because the
    /// OCCT tessellation tags its parts; the toy's mesh has no names. The shared triad
    /// must still arm for that selection — otherwise the Part edition shows no gizmo at
    /// all while the toy shows one, which is exactly what happened.
    #[test]
    fn the_shared_move_gizmo_claims_a_named_sub_element_too() {
        let stage = stage_with_a_box();
        let gizmo = base_contributions()
            .gizmo_for(&SelectionRef::element(7, "Face1"), &stage)
            .expect("a face selection arms the triad for its body");
        assert_eq!(gizmo.target_id(), 7);
    }
}
