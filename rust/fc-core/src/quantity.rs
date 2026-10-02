//! A `Quantity` (value + unit) with FreeCAD's unit-expression parser and
//! arithmetic.
//!
//! A `Quantity` stores a value expressed in its `unit`. Construction (parsing,
//! `Quantity::new`) *normalizes* to the canonical internal unit for the
//! signature (mm/kg/s/A/K/mol/cd/deg), so FreeCAD's `.Value` matches. The one
//! exception is `in_unit`, which builds a non-normalized quantity (used by
//! `getValueAs`) so its value is expressed in the requested unit.

use std::fmt;
use std::str::FromStr;

use crate::unit::{Signature, Unit};

#[derive(Debug, Clone, Copy)]
pub struct Quantity {
    /// Value expressed in `unit`.
    value: f64,
    unit: Unit,
}

impl Quantity {
    /// Construct from a value expressed in `unit`, normalized to canonical.
    pub fn new(value: f64, unit: Unit) -> Self {
        Quantity {
            value: value * unit.scale,
            unit: Unit { sig: unit.sig, scale: 1.0 },
        }
    }

    /// A dimensionless quantity.
    pub fn dimensionless(value: f64) -> Self {
        Quantity { value, unit: Unit::Dimensionless }
    }

    /// A non-normalized quantity: value expressed in `unit` (for `getValueAs`).
    pub fn in_unit(&self, unit: Unit) -> Quantity {
        Quantity { value: self.canonical_value() / unit.scale, unit }
    }

    /// FreeCAD's `.Value`: the value in this quantity's (current) unit.
    pub fn value(&self) -> f64 {
        self.value
    }

    /// The value in the canonical internal unit (mm for lengths).
    pub fn canonical_value(&self) -> f64 {
        self.value * self.unit.scale
    }

    /// Back-compat alias: internal value (mm for lengths).
    pub fn value_mm(&self) -> f64 {
        self.canonical_value()
    }

    pub fn sig(&self) -> Signature {
        self.unit.sig
    }

    pub fn unit(&self) -> Unit {
        self.unit
    }

    /// Value converted to another (same-signature) unit.
    pub fn value_in(&self, unit: Unit) -> f64 {
        self.canonical_value() / unit.scale
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        Parser::new(s).parse()
    }

    /// A canonical, parseable string (stable under round-trip).
    pub fn user_string(&self) -> String {
        let name = canonical_name(self.unit.sig);
        let v = self.canonical_value();
        if name.is_empty() {
            format!("{}", v)
        } else {
            format!("{} {}", v, name)
        }
    }

    pub fn add(&self, other: &Quantity) -> Result<Quantity, String> {
        if self.unit.sig != other.unit.sig {
            return Err("incompatible units".to_string());
        }
        Ok(Quantity { value: self.canonical_value() + other.canonical_value(), unit: self.unit })
    }

    pub fn sub(&self, other: &Quantity) -> Result<Quantity, String> {
        if self.unit.sig != other.unit.sig {
            return Err("incompatible units".to_string());
        }
        Ok(Quantity { value: self.canonical_value() - other.canonical_value(), unit: self.unit })
    }

    pub fn mul(&self, other: &Quantity) -> Quantity {
        Quantity { value: self.canonical_value() * other.canonical_value(), unit: self.unit.mul(&other.unit) }
    }

    pub fn div(&self, other: &Quantity) -> Quantity {
        Quantity { value: self.canonical_value() / other.canonical_value(), unit: self.unit.div(&other.unit) }
    }

    pub fn powi(&self, e: i32) -> Quantity {
        Quantity { value: self.canonical_value().powi(e), unit: self.unit.pow(e) }
    }

    pub fn neg(&self) -> Quantity {
        Quantity { value: -self.canonical_value(), unit: self.unit }
    }
}

impl PartialEq for Quantity {
    fn eq(&self, other: &Self) -> bool {
        self.unit.sig == other.unit.sig
            && (self.canonical_value() - other.canonical_value()).abs() < 1e-12
    }
}

impl FromStr for Quantity {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl fmt::Display for Quantity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.user_string())
    }
}

/// Parse a (possibly compound) unit expression such as `"F/m"` or `"mm^2"`
/// into a `Unit`. Returns `None` if it is not a pure unit expression.
pub fn parse_unit(s: &str) -> Option<Unit> {
    let mut p = Parser::new(s);
    let (value, unit) = p.parse_expr().ok()?;
    p.skip_ws();
    if p.peek().is_some() {
        return None;
    }
    if (value - 1.0).abs() > 1e-12 {
        return None; // not a pure unit expression
    }
    Some(unit)
}

/// A canonical string for a signature using the internal base units.
pub fn canonical_name(sig: Signature) -> String {
    const NAMES: [&str; 8] = ["mm", "kg", "s", "A", "K", "mol", "cd", "deg"];
    let mut pos: Vec<(&str, i8)> = Vec::new();
    let mut neg: Vec<(&str, i8)> = Vec::new();
    for i in 0..8 {
        let e = sig[i];
        if e > 0 {
            pos.push((NAMES[i], e));
        } else if e < 0 {
            neg.push((NAMES[i], -e));
        }
    }
    if pos.is_empty() && neg.is_empty() {
        return String::new();
    }
    let term = |name: &str, e: i8| {
        if e == 1 {
            name.to_string()
        } else {
            format!("{}^{}", name, e)
        }
    };
    let pos_s = pos.iter().map(|(n, e)| term(n, *e)).collect::<Vec<_>>().join("*");
    if neg.is_empty() {
        return pos_s;
    }
    let neg_s = neg.iter().map(|(n, e)| term(n, *e)).collect::<Vec<_>>().join("*");
    if pos.is_empty() {
        format!("1/({})", neg_s)
    } else {
        format!("{}/({})", pos_s, neg_s)
    }
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

struct Parser {
    chars: Vec<char>,
    pos: usize,
}

impl Parser {
    fn new(s: &str) -> Self {
        Parser { chars: s.chars().collect(), pos: 0 }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek2(&self) -> Option<char> {
        self.chars.get(self.pos + 1).copied()
    }

    fn next(&mut self) -> Option<char> {
        let c = self.peek();
        if c.is_some() {
            self.pos += 1;
        }
        c
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(c) if c.is_whitespace()) {
            self.pos += 1;
        }
    }

    fn parse(&mut self) -> Result<Quantity, String> {
        let (value, unit) = self.parse_expr()?;
        self.skip_ws();
        if self.peek().is_some() {
            return Err(format!("unexpected character '{}'", self.peek().unwrap()));
        }
        Ok(Quantity::new(value, unit))
    }

    fn parse_expr(&mut self) -> Result<(f64, Unit), String> {
        let (mut value, unit) = self.parse_term()?;
        loop {
            self.skip_ws();
            match self.peek() {
                Some('+') | Some('-') => {
                    let op = self.next().unwrap();
                    let (v2, u2) = self.parse_term()?;
                    if unit.sig != u2.sig {
                        return Err("incompatible units".to_string());
                    }
                    let v2 = v2 * u2.scale / unit.scale;
                    value = if op == '+' { value + v2 } else { value - v2 };
                }
                _ => break,
            }
        }
        Ok((value, unit))
    }

    fn parse_term(&mut self) -> Result<(f64, Unit), String> {
        let (mut value, mut unit) = self.parse_unary()?;
        loop {
            self.skip_ws();
            match self.peek() {
                Some('*') => {
                    self.next();
                    let (v2, u2) = self.parse_unary()?;
                    value *= v2;
                    unit = unit.mul(&u2);
                }
                Some('/') => {
                    self.next();
                    let (v2, u2) = self.parse_unary()?;
                    value /= v2;
                    unit = unit.div(&u2);
                }
                Some(c) if starts_primary(c) => {
                    // implicit multiplication: "10 m", "1psi", "2*pi rad"
                    let (v2, u2) = self.parse_unary()?;
                    value *= v2;
                    unit = unit.mul(&u2);
                }
                _ => break,
            }
        }
        Ok((value, unit))
    }

    fn parse_unary(&mut self) -> Result<(f64, Unit), String> {
        self.skip_ws();
        if self.peek() == Some('-') {
            self.next();
            let (v, u) = self.parse_unary()?;
            return Ok((-v, u));
        }
        self.parse_power()
    }

    fn parse_power(&mut self) -> Result<(f64, Unit), String> {
        let (mut v, mut u) = self.parse_primary()?;
        self.skip_ws();
        if self.peek() == Some('^') {
            self.next();
            self.skip_ws();
            let e = self.parse_int()?;
            v = v.powi(e);
            u = u.pow(e);
        }
        Ok((v, u))
    }

    fn parse_primary(&mut self) -> Result<(f64, Unit), String> {
        self.skip_ws();
        match self.peek() {
            Some('(') => {
                self.next();
                let r = self.parse_expr()?;
                self.skip_ws();
                if self.next() != Some(')') {
                    return Err("expected ')'".to_string());
                }
                Ok(r)
            }
            Some(c) if c.is_ascii_digit() || c == '.' => {
                let (v, u) = self.parse_number()?;
                self.skip_ws();
                // Feet-inches building notation: `N'` or `N'(expr)"`.
                if self.peek() == Some('\'') {
                    self.next();
                    let ft = Unit::parse("ft").unwrap();
                    self.skip_ws();
                    if self.peek() == Some('(') {
                        let (iv, iu) = self.parse_primary()?;
                        if iu.sig != [0; 8] {
                            return Err("inches must be dimensionless".to_string());
                        }
                        self.skip_ws();
                        if self.next() != Some('"') {
                            return Err("expected '\"' after inches".to_string());
                        }
                        let inch = Unit::parse("in").unwrap();
                        // 1 ft = 304.8 mm, 1 in = 25.4 mm.
                        let total = v * ft.scale + iv * inch.scale;
                        return Ok((total, Unit::Millimeter));
                    }
                    return Ok((v, ft));
                }
                Ok((v, u))
            }
            Some(c) if c.is_alphabetic() || c == '°' || c == 'µ' || c == 'μ' => {
                let ident = self.read_ident();
                if let Some(f) = function(&ident) {
                    self.skip_ws();
                    if self.next() != Some('(') {
                        return Err(format!("expected '(' after {}", ident));
                    }
                    let (v, u) = self.parse_expr()?;
                    self.skip_ws();
                    if self.next() != Some(')') {
                        return Err("expected ')'".to_string());
                    }
                    if u.sig != [0; 8] {
                        return Err(format!("{} expects a dimensionless argument", ident));
                    }
                    Ok((f(v), Unit::Dimensionless))
                } else if ident == "pi" {
                    Ok((std::f64::consts::PI, Unit::Dimensionless))
                } else {
                    match Unit::parse(&ident) {
                        Some(u) => Ok((1.0, u)),
                        None => Err(format!("unknown unit '{}'", ident)),
                    }
                }
            }
            Some(c) => Err(format!("unexpected character '{}'", c)),
            None => Err("unexpected end of expression".to_string()),
        }
    }

    fn parse_number(&mut self) -> Result<(f64, Unit), String> {
        let start = self.pos;
        while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
            self.pos += 1;
        }
        // fraction: "3/8"
        if self.peek() == Some('/') && matches!(self.peek2(), Some(c) if c.is_ascii_digit()) {
            self.pos += 1; // consume '/'
            let num: f64 = self.chars[start..self.pos - 1].iter().collect::<String>().parse().unwrap();
            let dstart = self.pos;
            while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                self.pos += 1;
            }
            let den: f64 = self.chars[dstart..self.pos].iter().collect::<String>().parse().unwrap();
            return Ok((num / den, Unit::Dimensionless));
        }
        // decimal part
        if self.peek() == Some('.') {
            self.pos += 1;
            while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                self.pos += 1;
            }
        }
        // exponent
        if matches!(self.peek(), Some('e') | Some('E')) {
            self.pos += 1;
            if matches!(self.peek(), Some('+') | Some('-')) {
                self.pos += 1;
            }
            while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                self.pos += 1;
            }
        }
        let text: String = self.chars[start..self.pos].iter().collect();
        let value: f64 = text.parse().map_err(|e| format!("invalid number '{}': {}", text, e))?;
        Ok((value, Unit::Dimensionless))
    }

    fn parse_int(&mut self) -> Result<i32, String> {
        self.skip_ws();
        let mut neg = false;
        if self.peek() == Some('-') {
            neg = true;
            self.next();
        }
        let start = self.pos;
        while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
            self.pos += 1;
        }
        let text: String = self.chars[start..self.pos].iter().collect();
        let v: i32 = text.parse().map_err(|_| "expected integer exponent".to_string())?;
        Ok(if neg { -v } else { v })
    }

    fn read_ident(&mut self) -> String {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c.is_alphabetic() || c == '°' || c == 'µ' || c == 'μ' {
                self.pos += 1;
            } else {
                break;
            }
        }
        self.chars[start..self.pos].iter().collect()
    }
}

fn starts_primary(c: char) -> bool {
    c.is_ascii_digit() || c == '.' || c == '(' || c.is_alphabetic() || c == '°' || c == 'µ' || c == 'μ'
}

fn function(name: &str) -> Option<fn(f64) -> f64> {
    Some(match name {
        "sin" => f64::sin,
        "cos" => f64::cos,
        "tan" => f64::tan,
        "sqrt" => f64::sqrt,
        "asin" => f64::asin,
        "acos" => f64::acos,
        "atan" => f64::atan,
        "abs" => f64::abs,
        "exp" => f64::exp,
        "ln" | "log" => f64::ln,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn val(s: &str) -> f64 {
        s.parse::<Quantity>().unwrap().canonical_value()
    }

    #[test]
    fn parses_simple_units() {
        assert_eq!(val("10 m"), 10000.0);
        assert_eq!(val("2.5 in"), 63.5);
        assert_eq!(val("1 m"), 1000.0);
        assert!((val("3/8 in") - 9.525).abs() < 1e-12);
    }

    #[test]
    fn parses_compound() {
        assert_eq!(val("m^2*kg*s^-3*A^-2"), 1e6);
        assert_eq!(val("(m^2*kg)/(A^2*s^3)"), 1e6);
        assert!((val("100 km/h") - 27777.77777777).abs() < 1e-6);
    }

    #[test]
    fn parses_angle_and_functions() {
        assert!((val("2*pi rad") - 360.0).abs() < 1e-9);
        assert!((val("sin(pi)") - std::f64::consts::PI.sin()).abs() < 1e-9);
        assert!((val("cos(pi)") - (-1.0)).abs() < 1e-9);
    }

    #[test]
    fn arithmetic() {
        let a: Quantity = "1 m".parse().unwrap();
        let b: Quantity = "50 cm".parse().unwrap();
        assert_eq!(a.add(&b).unwrap().canonical_value(), 1500.0);
        let p: Quantity = "1 m".parse().unwrap();
        let q: Quantity = "1 s".parse().unwrap();
        assert_eq!(p.div(&q).canonical_value(), 1000.0); // mm/s
    }

    #[test]
    fn feet_inches_notation() {
        assert!((val("1'(3+7/16)\"") - 392.1125).abs() < 1e-9);
    }

    #[test]
    fn parses_compound_unit_expression() {
        assert!(parse_unit("F/m").is_some());
        assert!(parse_unit("mm^2").is_some());
        assert!(parse_unit("kg/(m*s^2)").is_some());
        assert!(parse_unit("2 m").is_none()); // not a pure unit
    }
}
