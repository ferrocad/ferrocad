//! Keyboard accelerators for workbench commands (A4).
//!
//! A command's `GetResources()["Accel"]` is an upstream-style string (`"Ctrl+Shift+S"`,
//! `"Del"`). The shell parses it once per command refresh and matches it against the
//! key events that bubble to the window root, so a shortcut runs exactly the same
//! `run_command` path as a click.
//!
//! The grammar is the documented subset: modifiers joined with `+`/`-`, then one key.
//! Upstream's multi-key sequences (`"S, H"`) are not parsed. A **printable** key
//! without Ctrl/Alt/Cmd is deliberately not bound globally: it would shadow typing in
//! the console and the property editor (see [`Accelerator::is_global`]).

use gpui::Keystroke;

/// A parsed accelerator.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Accelerator {
    /// The gpui key name (`"s"`, `"delete"`, `"f5"`).
    pub key: String,
    pub control: bool,
    pub alt: bool,
    pub shift: bool,
    /// Cmd on macOS, the Windows/Super key elsewhere.
    pub platform: bool,
}

impl Accelerator {
    /// Parse an upstream `Accel` string.
    ///
    /// Accepted modifiers: `Ctrl`/`Control`, `Shift`, `Alt`/`Option`, and
    /// `Cmd`/`Command`/`Meta`/`Super`/`Win`. Keys are matched case-insensitively;
    /// `Del`/`Esc`/`Return` are mapped to gpui's names. Returns `None` for syntax the
    /// subset does not cover (no key, two keys, an unknown modifier).
    pub fn parse(accel: &str) -> Option<Self> {
        let mut accelerator = Self::default();
        let mut tokens = accel
            .split(['+', '-'])
            .map(str::trim)
            .filter(|token| !token.is_empty());
        loop {
            let token = tokens.next()?;
            match Modifier::parse(token) {
                Some(Modifier::Control) => accelerator.control = true,
                Some(Modifier::Shift) => accelerator.shift = true,
                Some(Modifier::Alt) => accelerator.alt = true,
                Some(Modifier::Platform) => accelerator.platform = true,
                None => {
                    // The key must be last: anything after it is upstream's sequence
                    // grammar, which this subset does not bind.
                    if tokens.next().is_some() {
                        return None;
                    }
                    accelerator.key = canonical_key(token)?;
                    return Some(accelerator);
                }
            }
        }
    }

    /// Whether the accelerator is safe to bind window-wide.
    ///
    /// A printable key with no Ctrl/Alt/Cmd would fire while the user is typing, so
    /// only chorded accelerators and named keys (`Del`, `F5`) are global. A
    /// `Shift`-only letter is printable and therefore not global yet.
    pub fn is_global(&self) -> bool {
        self.control || self.alt || self.platform || !is_printable(&self.key)
    }

    /// Whether `keystroke` is this accelerator, modifiers included.
    pub fn matches(&self, keystroke: &Keystroke) -> bool {
        self.key == keystroke.key
            && self.control == keystroke.modifiers.control
            && self.alt == keystroke.modifiers.alt
            && self.shift == keystroke.modifiers.shift
            && self.platform == keystroke.modifiers.platform
    }
}

enum Modifier {
    Control,
    Shift,
    Alt,
    Platform,
}

impl Modifier {
    fn parse(token: &str) -> Option<Self> {
        match token.trim().to_ascii_lowercase().as_str() {
            "ctrl" | "control" => Some(Self::Control),
            "shift" => Some(Self::Shift),
            "alt" | "option" | "opt" => Some(Self::Alt),
            "cmd" | "command" | "meta" | "super" | "win" => Some(Self::Platform),
            _ => None,
        }
    }
}

fn canonical_key(token: &str) -> Option<String> {
    let lower = token.trim().to_ascii_lowercase();
    let key = match lower.as_str() {
        "del" | "delete" => "delete",
        "esc" | "escape" => "escape",
        "return" | "enter" => "enter",
        "bs" | "backspace" => "backspace",
        "pgup" => "pageup",
        "pgdn" => "pagedown",
        other => other,
    };
    let known = key.chars().count() == 1
        || matches!(
            key,
            "delete"
                | "escape"
                | "enter"
                | "backspace"
                | "space"
                | "tab"
                | "up"
                | "down"
                | "left"
                | "right"
                | "home"
                | "end"
                | "pageup"
                | "pagedown"
        )
        || (key.starts_with('f') && key[1..].parse::<u8>().is_ok_and(|n| (1..=24).contains(&n)));
    known.then(|| key.to_string())
}

fn is_printable(key: &str) -> bool {
    key.chars().count() == 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::Modifiers;

    fn keystroke(key: &str, modifiers: Modifiers) -> Keystroke {
        Keystroke {
            key: key.to_string(),
            modifiers,
            key_char: None,
        }
    }

    #[test]
    fn it_parses_modifiers_and_a_key() {
        let accel = Accelerator::parse("Ctrl+Shift+S").expect("a chord");
        assert_eq!(accel.key, "s");
        assert!(accel.control);
        assert!(accel.shift);
        assert!(!accel.alt);
        assert!(!accel.platform);
        assert!(accel.is_global());
    }

    #[test]
    fn it_maps_named_keys() {
        assert_eq!(Accelerator::parse("Del").unwrap().key, "delete");
        assert_eq!(Accelerator::parse("Esc").unwrap().key, "escape");
        assert_eq!(Accelerator::parse("F5").unwrap().key, "f5");
        assert!(Accelerator::parse("Del").unwrap().is_global());
        assert!(Accelerator::parse("Ctrl+Z").unwrap().is_global());
    }

    #[test]
    fn a_shift_only_letter_is_not_bound_globally() {
        let accel = Accelerator::parse("Shift+S").expect("parses");
        assert!(!accel.is_global(), "it would shadow typing");
        assert!(Accelerator::parse("Alt+S").unwrap().is_global());
        assert!(Accelerator::parse("Cmd+S").unwrap().is_global());
    }

    #[test]
    fn it_rejects_what_the_subset_does_not_cover() {
        assert!(Accelerator::parse("").is_none());
        assert!(Accelerator::parse("Ctrl+").is_none());
        assert!(Accelerator::parse("Ctrl+Shift").is_none(), "no key");
        assert!(Accelerator::parse("S, H").is_none(), "a sequence");
        assert!(Accelerator::parse("Hyper+S").is_none(), "unknown modifier");
        assert!(Accelerator::parse("Ctrl+S+H").is_none(), "two keys");
    }

    #[test]
    fn it_matches_the_exact_chord() {
        let accel = Accelerator::parse("Ctrl+Shift+K").unwrap();
        let mut modifiers = Modifiers::default();
        modifiers.control = true;
        modifiers.shift = true;
        assert!(accel.matches(&keystroke("k", modifiers)));

        let mut control_only = Modifiers::default();
        control_only.control = true;
        assert!(
            !accel.matches(&keystroke("k", control_only)),
            "shift differs"
        );
        assert!(
            !accel.matches(&keystroke("j", modifiers)),
            "the key differs"
        );
    }
}
