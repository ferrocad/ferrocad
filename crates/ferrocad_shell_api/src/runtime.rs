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

    /// The runtime named by `FERROCAD_SCRIPT_HOST`, if it is set.
    ///
    /// The desktop's switch is a flag (`--script-host=`), but an environment variable is
    /// what a test harness or a wrapper script can set without rewriting a command line.
    pub fn from_env() -> Result<Option<Self>, String> {
        match std::env::var("FERROCAD_SCRIPT_HOST") {
            Ok(name) if !name.trim().is_empty() => {
                Self::from_name(name.trim()).map(Some)
            }
            _ => Ok(None),
        }
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
