//! Which scripting configuration a host installs.
//!
//! Two axes live here. [`Runtime`] is the *switch* vocabulary — the name a URL or a command
//! line uses — and it names a language and an engine at once (`cpython` is Python with the
//! embedded CPython, `pywasm` is Python with RustPython). [`Language`] is the durable half:
//! what a workbench author targets, with [`Platform`] choosing which engine implements it
//! on each host (B8). `plain` is "no language host installed", not a fourth language.
//!
//! The names are the same everywhere — `plain`, `cpython`, `pywasm`, `js` — and so is
//! the parsing, so the browser's `?runtime=` and the desktop's `--script-host=` /
//! `FERROCAD_SCRIPT_HOST` cannot drift. The *composition* stays with the host, because
//! only the host knows what it linked; this module is only the vocabulary.
//!
//! `python` is the Python *this host* has: CPython natively, RustPython in the browser.
//! The canonical names are never ambiguous. A browser compiles this crate for the host
//! when it tests its own switch, so its parser folds `python` to `pywasm` itself rather
//! than relying on the target: the browser has exactly one Python, and that is the one a
//! page means.

use std::fmt;

/// One scripting half a host can install.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Runtime {
    /// The document half alone: no interpreter, no page. Every host has this one.
    Plain,
    /// The embedded CPython (`ferrocad_shell_python`). Native only: the bindings are a
    /// PyO3 C-ABI extension.
    Cpython,
    /// RustPython (`ferrocad_shell_pywasm`). Plain Rust, so both hosts can serve it.
    Pywasm,
    /// The page's ES module (`ferrocad_shell_js`): the browser's `window.__ferrocadJs`,
    /// or a desktop page over an embedded engine.
    Js,
}

impl Runtime {
    /// Every name, in the order a help line should list them.
    pub const ALL: [Runtime; 4] = [Runtime::Plain, Runtime::Cpython, Runtime::Pywasm, Runtime::Js];

    /// The name the switch uses and the host logs.
    pub fn id(self) -> &'static str {
        match self {
            Runtime::Plain => "plain",
            Runtime::Cpython => "cpython",
            Runtime::Pywasm => "pywasm",
            Runtime::Js => "js",
        }
    }

    /// Whether a host on this target can serve it at all.
    ///
    /// Only CPython is target-gated: `ferrocad_py` is a native C-ABI extension, so no
    /// wasm build can link it. The other three are a question of what a host chose to
    /// link, which is not something this module can answer.
    pub fn on_this_target(self) -> bool {
        match self {
            Runtime::Cpython => !cfg!(target_family = "wasm"),
            _ => true,
        }
    }

    /// A name from a switch (`?runtime=js`, `--script-host=js`), with the aliases a
    /// hand-written URL or command line tends to use.
    pub fn from_name(name: &str) -> Result<Self, String> {
        let runtime = match name {
            "plain" | "none" | "rust" => Runtime::Plain,
            "cpython" => Runtime::Cpython,
            // `python` is the Python this host has, so the browser's name for
            // RustPython is the one a page would use.
            "python" | "py" if cfg!(target_family = "wasm") => Runtime::Pywasm,
            "python" | "py" => Runtime::Cpython,
            "pywasm" | "rustpython" => Runtime::Pywasm,
            "js" | "javascript" => Runtime::Js,
            other => {
                return Err(format!(
                    "unknown runtime {other:?}; expected plain, cpython, pywasm or js"
                ));
            }
        };
        Ok(runtime)
    }

    /// The list named by `FERROCAD_SCRIPT_HOST`, if it is set.
    ///
    /// The desktop's switch is a flag (`--script-host=`), but an environment variable is
    /// what a test harness or a wrapper script can set without rewriting a command line.
    pub fn from_env_list() -> Result<Option<Vec<Self>>, String> {
        match std::env::var("FERROCAD_SCRIPT_HOST") {
            Ok(names) if !names.trim().is_empty() => Self::parse_list(&names).map(Some),
            _ => Ok(None),
        }
    }

    /// Parse a switch value that may name **several** hosts: `cpython,js`.
    ///
    /// The first name is the primary: it owns the console, the document lifecycle and the
    /// tools, and the rest answer commands, the display hook and the document's events.
    /// A list names at most one host per [`Language`] — `cpython,pywasm` is two engines for
    /// one language, not two hosts — and `plain` cannot be combined, because it is the
    /// absence of a host rather than one.
    pub fn parse_list(names: &str) -> Result<Vec<Self>, String> {
        let names = names.trim();
        if names.is_empty() {
            return Err("no runtime named; expected plain, cpython, pywasm or js".to_string());
        }
        let mut runtimes = Vec::new();
        for name in names.split(',') {
            runtimes.push(Self::from_name(name.trim())?);
        }
        Self::validate_list(&runtimes)?;
        Ok(runtimes)
    }

    /// The rule [`parse_list`](Self::parse_list) applies, for a list assembled elsewhere.
    pub fn validate_list(runtimes: &[Self]) -> Result<(), String> {
        if runtimes.len() > 1 && runtimes.contains(&Runtime::Plain) {
            return Err(
                "plain is the document half alone; it cannot be combined with a host".to_string(),
            );
        }
        let mut seen: Vec<(Language, Self)> = Vec::new();
        for runtime in runtimes {
            let Some(language) = Language::of(*runtime) else {
                continue;
            };
            if let Some((_, first)) = seen.iter().find(|(seen, _)| *seen == language) {
                return Err(format!(
                    "{first} and {runtime} are both {}; install one host per language",
                    language.id()
                ));
            }
            seen.push((language, *runtime));
        }
        Ok(())
    }
}

/// What a mod package declares by its entry file.
///
/// B9: a mod directory says what it needs — `InitGui.py`, `InitGui.js` or `addon.wasm` —
/// so a host set can be derived instead of typed. The names are the ones each loader
/// already looks for, not a new manifest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EntryKind {
    /// `Init.py`/`InitGui.py`: a Python workbench.
    Python,
    /// `InitGui.js`: the page's module.
    JavaScript,
    /// `addon.wasm`: a sandboxed bundle.
    Wasm,
}

impl EntryKind {
    /// Every kind, in the order a host set derives them.
    pub const ALL: [EntryKind; 3] = [EntryKind::Python, EntryKind::JavaScript, EntryKind::Wasm];

    /// The kind an entry file name declares, if any.
    pub fn of_file_name(name: &str) -> Option<Self> {
        match name {
            "Init.py" | "InitGui.py" => Some(EntryKind::Python),
            "InitGui.js" => Some(EntryKind::JavaScript),
            "addon.wasm" => Some(EntryKind::Wasm),
            _ => None,
        }
    }

    /// The name a host logs.
    pub fn id(self) -> &'static str {
        match self {
            EntryKind::Python => "python",
            EntryKind::JavaScript => "javascript",
            EntryKind::Wasm => "wasm",
        }
    }
}

impl Runtime {
    /// The script hosts `entries` need on `platform`, primary first.
    ///
    /// Python is the primary when it is present: it is the FreeCAD-shaped reference and
    /// owns a finer surface than the page. A wasm bundle asks for the page half only in the
    /// browser, where `loader.js` is driven by it — on the desktop `wasmi` needs no script
    /// half at all, so a wasm-only mod set derives nothing here and the bundles ride beside
    /// whatever else was chosen.
    pub fn hosts_for(entries: &[EntryKind], platform: Platform) -> Vec<Self> {
        let has = |kind: EntryKind| entries.contains(&kind);
        let mut hosts = Vec::new();
        if has(EntryKind::Python) {
            hosts.push(match platform {
                Platform::Desktop => Runtime::Cpython,
                Platform::Browser => Runtime::Pywasm,
            });
        }
        if has(EntryKind::JavaScript)
            || (platform == Platform::Browser && has(EntryKind::Wasm))
        {
            hosts.push(Runtime::Js);
        }
        hosts
    }
}

impl fmt::Display for Runtime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.id())
    }
}

/// One scripting language a host can run.
///
/// A mod author targets a *language*; the host picks the engine (D10). Pyodide is the test
/// case for the split: it is a second engine for [`Language::Python`] on the browser, not a
/// fifth [`Runtime`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Language {
    /// CPython or RustPython, whichever the host linked.
    Python,
    /// The page's own module (browser) or QuickJS (desktop).
    JavaScript,
    /// A sandboxed bundle: `wasmi` on the desktop, `WebAssembly` in the page.
    Wasm,
}

impl Language {
    /// Every language, in the order a help line should list them.
    pub const ALL: [Language; 3] = [Language::Python, Language::JavaScript, Language::Wasm];

    /// The name a host logs and a mod directory is described by.
    pub fn id(self) -> &'static str {
        match self {
            Language::Python => "python",
            Language::JavaScript => "javascript",
            Language::Wasm => "wasm",
        }
    }

    /// The language `runtime` installs; `None` for [`Runtime::Plain`], which installs none.
    pub fn of(runtime: Runtime) -> Option<Self> {
        match runtime {
            Runtime::Plain => None,
            Runtime::Cpython | Runtime::Pywasm => Some(Language::Python),
            Runtime::Js => Some(Language::JavaScript),
        }
    }

    /// The switch values that install this language on `platform`, in preference order.
    ///
    /// The match is exhaustive over the cell, so a new [`Language`] or [`Platform`] does
    /// not compile until it is answered here. [`Language::Wasm`] has no value on either
    /// platform: a bundle rides *beside* the script half (`--addons` / `?addons=`) rather
    /// than replacing it, which is the B7e decision — wasm is a package format, not a
    /// runtime.
    pub fn runtimes(self, platform: Platform) -> &'static [Runtime] {
        use Language::{JavaScript, Python, Wasm};
        use Platform::{Browser, Desktop};
        match (self, platform) {
            (Python, Desktop) => &[Runtime::Cpython, Runtime::Pywasm],
            (Python, Browser) => &[Runtime::Pywasm],
            (JavaScript, Desktop) => &[Runtime::Js],
            (JavaScript, Browser) => &[Runtime::Js],
            (Wasm, Desktop) => &[],
            (Wasm, Browser) => &[],
        }
    }
}

/// The two hosts a language can run in.
///
/// Not a target list: CPython is native-only and Pyodide would be browser-only, so the
/// `(Language, Platform)` cell is what decides an engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Platform {
    /// The native window (`ferrocad_parametric`, `ferrocad_toy`).
    Desktop,
    /// The browser module (`ferrocad_web_gpui`).
    Browser,
}

impl Platform {
    /// Both hosts, in the order a figure draws them.
    pub const ALL: [Platform; 2] = [Platform::Desktop, Platform::Browser];

    /// The name a host logs.
    pub fn id(self) -> &'static str {
        match self {
            Platform::Desktop => "desktop",
            Platform::Browser => "browser",
        }
    }

    /// The platform this build is for — the same test the hosts already make.
    pub fn current() -> Self {
        if cfg!(target_family = "wasm") {
            Platform::Browser
        } else {
            Platform::Desktop
        }
    }
}
