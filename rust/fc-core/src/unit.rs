//! FreeCAD-style units: an 8-dimensional signature (plus angle) and a scale.
//!
//! The *internal* base units are `mm` (length), `kg` (mass), `s` (time), `A`,
//! `K`, `mol`, `cd`, and `deg` (angle). A `Unit` is `(signature, scale)` where
//! `1 unit = scale * internal_base(signature)`.

#![allow(non_upper_case_globals)]

use std::collections::HashMap;
use std::f64::consts::PI;
use std::sync::OnceLock;

pub type Signature = [i8; 8];

const RAD_TO_DEG: f64 = 180.0 / PI;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Unit {
    pub sig: Signature,
    pub scale: f64,
}

impl Unit {
    pub const Dimensionless: Unit = Unit { sig: [0; 8], scale: 1.0 };
    pub const Millimeter: Unit = Unit { sig: [1, 0, 0, 0, 0, 0, 0, 0], scale: 1.0 };

    pub fn is_dimensionless(&self) -> bool {
        self.sig == [0; 8]
    }

    pub fn mul(&self, o: &Unit) -> Unit {
        let mut sig = [0i8; 8];
        for i in 0..8 {
            sig[i] = self.sig[i] + o.sig[i];
        }
        Unit { sig, scale: self.scale * o.scale }
    }

    pub fn div(&self, o: &Unit) -> Unit {
        let mut sig = [0i8; 8];
        for i in 0..8 {
            sig[i] = self.sig[i] - o.sig[i];
        }
        Unit { sig, scale: self.scale / o.scale }
    }

    pub fn pow(&self, e: i32) -> Unit {
        let mut sig = [0i8; 8];
        for i in 0..8 {
            sig[i] = self.sig[i] * e as i8;
        }
        Unit { sig, scale: self.scale.powi(e) }
    }

    pub fn parse(name: &str) -> Option<Unit> {
        lookup(name)
    }
}

/// Build a unit from its SI expression `x * m^l kg^m s^t A^i K^th mol^n cd^j rad^a`.
fn si(l: i8, m: i8, t: i8, i: i8, th: i8, n: i8, j: i8, a: i8, x: f64) -> Unit {
    Unit {
        sig: [l, m, t, i, th, n, j, a],
        scale: x * 1000f64.powi(l as i32) * RAD_TO_DEG.powi(a as i32),
    }
}

fn table() -> &'static HashMap<&'static str, Unit> {
    static T: OnceLock<HashMap<&'static str, Unit>> = OnceLock::new();
    T.get_or_init(|| {
        let mut m: HashMap<&'static str, Unit> = HashMap::new();
        // SI base
        m.insert("m", si(1, 0, 0, 0, 0, 0, 0, 0, 1.0));
        m.insert("g", si(0, 1, 0, 0, 0, 0, 0, 0, 1e-3));
        m.insert("s", si(0, 0, 1, 0, 0, 0, 0, 0, 1.0));
        m.insert("A", si(0, 0, 0, 1, 0, 0, 0, 0, 1.0));
        m.insert("K", si(0, 0, 0, 0, 1, 0, 0, 0, 1.0));
        m.insert("mol", si(0, 0, 0, 0, 0, 1, 0, 0, 1.0));
        m.insert("cd", si(0, 0, 0, 0, 0, 0, 1, 0, 1.0));
        m.insert("rad", si(0, 0, 0, 0, 0, 0, 0, 1, 1.0));
        // common mass / time aliases
        m.insert("kg", si(0, 1, 0, 0, 0, 0, 0, 0, 1.0));
        m.insert("t", si(0, 1, 0, 0, 0, 0, 0, 0, 1e3));
        m.insert("min", si(0, 0, 1, 0, 0, 0, 0, 0, 60.0));
        m.insert("h", si(0, 0, 1, 0, 0, 0, 0, 0, 3600.0));
        // angle
        m.insert("deg", si(0, 0, 0, 0, 0, 0, 0, 1, PI / 180.0));
        m.insert("°", si(0, 0, 0, 0, 0, 0, 0, 1, PI / 180.0));
        m.insert("gon", si(0, 0, 0, 0, 0, 0, 0, 1, PI / 200.0));
        // imperial length
        m.insert("in", si(1, 0, 0, 0, 0, 0, 0, 0, 0.0254));
        m.insert("ft", si(1, 0, 0, 0, 0, 0, 0, 0, 0.3048));
        m.insert("yd", si(1, 0, 0, 0, 0, 0, 0, 0, 0.9144));
        m.insert("mi", si(1, 0, 0, 0, 0, 0, 0, 0, 1609.344));
        m.insert("thou", si(1, 0, 0, 0, 0, 0, 0, 0, 2.54e-5));
        // imperial mass
        m.insert("lb", si(0, 1, 0, 0, 0, 0, 0, 0, 0.45359237));
        m.insert("oz", si(0, 1, 0, 0, 0, 0, 0, 0, 0.028349523125));
        // volume
        m.insert("l", si(3, 0, 0, 0, 0, 0, 0, 0, 1e-3));
        m.insert("L", si(3, 0, 0, 0, 0, 0, 0, 0, 1e-3));
        // derived SI
        m.insert("N", si(1, 1, -2, 0, 0, 0, 0, 0, 1.0));     // newton
        m.insert("Pa", si(-1, 1, -2, 0, 0, 0, 0, 0, 1.0));    // pascal
        m.insert("bar", si(-1, 1, -2, 0, 0, 0, 0, 0, 1e5));   // bar
        m.insert("J", si(2, 1, -2, 0, 0, 0, 0, 0, 1.0));      // joule
        m.insert("W", si(2, 1, -3, 0, 0, 0, 0, 0, 1.0));      // watt
        m.insert("V", si(2, 1, -3, -1, 0, 0, 0, 0, 1.0));     // volt
        m.insert("C", si(0, 0, 1, 1, 0, 0, 0, 0, 1.0));       // coulomb
        m.insert("F", si(-2, -1, 4, 2, 0, 0, 0, 0, 1.0));     // farad
        m.insert("Ohm", si(2, 1, -3, -2, 0, 0, 0, 0, 1.0));   // ohm
        m.insert("ohm", si(2, 1, -3, -2, 0, 0, 0, 0, 1.0));
        m.insert("S", si(-2, -1, 3, 2, 0, 0, 0, 0, 1.0));     // siemens
        m.insert("Hz", si(0, 0, -1, 0, 0, 0, 0, 0, 1.0));     // hertz
        m.insert("T", si(0, 1, -2, -1, 0, 0, 0, 0, 1.0));     // tesla
        m.insert("Wb", si(2, 1, -2, -1, 0, 0, 0, 0, 1.0));    // weber
        m.insert("H", si(2, 1, -2, -2, 0, 0, 0, 0, 1.0));     // henry
        m.insert("eV", si(2, 1, -2, 0, 0, 0, 0, 0, 1.602176634e-19));
        // pressure / stress aliases
        m.insert("psi", si(-1, 1, -2, 0, 0, 0, 0, 0, 6894.757293168));
        m.insert("ksi", si(-1, 1, -2, 0, 0, 0, 0, 0, 6894757.293168));
        m
    })
}

const PREFIXES: &[(&str, f64)] = &[
    ("da", 1e1),
    ("h", 1e2),
    ("k", 1e3),
    ("M", 1e6),
    ("G", 1e9),
    ("T", 1e12),
    ("P", 1e15),
    ("E", 1e18),
    ("d", 1e-1),
    ("c", 1e-2),
    ("m", 1e-3),
    ("u", 1e-6),
    ("µ", 1e-6),
    ("μ", 1e-6),
    ("n", 1e-9),
    ("p", 1e-12),
    ("f", 1e-15),
];

fn lookup(name: &str) -> Option<Unit> {
    if let Some(u) = table().get(name) {
        return Some(*u);
    }
    for (p, factor) in PREFIXES {
        if let Some(rest) = name.strip_prefix(p) {
            if !rest.is_empty() {
                if let Some(u) = table().get(rest) {
                    return Some(Unit { sig: u.sig, scale: u.scale * factor });
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_base_and_prefixed() {
        assert_eq!(lookup("m").unwrap().scale, 1000.0);
        assert_eq!(lookup("mm").unwrap().scale, 1.0);
        assert_eq!(lookup("km").unwrap().scale, 1e6);
        assert_eq!(lookup("in").unwrap().scale, 25.4);
        assert_eq!(lookup("kg").unwrap().scale, 1.0);
        assert_eq!(lookup("min").unwrap().scale, 60.0);
        assert_eq!(lookup("MPa").unwrap().scale, 1000.0);
    }

    #[test]
    fn angle_scales() {
        // 1 rad = 180/pi degrees; 1 deg = 1 degree; 1 gon = 0.9 degree.
        assert!((lookup("rad").unwrap().scale - RAD_TO_DEG).abs() < 1e-12);
        assert_eq!(lookup("deg").unwrap().scale, 1.0);
        assert_eq!(lookup("gon").unwrap().scale, 0.9);
    }

    #[test]
    fn arithmetic() {
        let m = lookup("m").unwrap();
        let s = lookup("s").unwrap();
        let mps = m.div(&s);
        assert_eq!(mps.sig, [1, 0, -1, 0, 0, 0, 0, 0]);
        assert_eq!(mps.scale, 1000.0);
    }
}
