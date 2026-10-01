//! A minimal `Quantity` (value + unit), stored internally in millimetres.

use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Millimeter,
    Centimeter,
    Meter,
    Inch,
    Foot,
    Dimensionless,
}

impl Unit {
    pub fn factor_to_mm(self) -> f64 {
        match self {
            Unit::Millimeter => 1.0,
            Unit::Centimeter => 10.0,
            Unit::Meter => 1000.0,
            Unit::Inch => 25.4,
            Unit::Foot => 304.8,
            Unit::Dimensionless => 1.0,
        }
    }

    fn parse(s: &str) -> Option<Unit> {
        Some(match s.to_ascii_lowercase().as_str() {
            "mm" => Unit::Millimeter,
            "cm" => Unit::Centimeter,
            "m" => Unit::Meter,
            "in" | "\"" => Unit::Inch,
            "ft" | "'" => Unit::Foot,
            _ => return None,
        })
    }
}

/// A value with a unit, e.g. `10 mm`. The value is stored in millimetres.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quantity {
    mm: f64,
}

impl Quantity {
    pub fn new(value: f64, unit: Unit) -> Self {
        Quantity {
            mm: value * unit.factor_to_mm(),
        }
    }

    pub fn value_mm(&self) -> f64 {
        self.mm
    }

    pub fn value_in(&self, unit: Unit) -> f64 {
        self.mm / unit.factor_to_mm()
    }
}

impl FromStr for Quantity {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        let idx = s.find(|c: char| {
            !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+' || c.is_whitespace())
        });
        let (num, unit) = match idx {
            Some(i) => (&s[..i], &s[i..]),
            None => (s, ""),
        };
        let value: f64 = num
            .trim()
            .parse()
            .map_err(|e| format!("invalid number in '{s}': {e}"))?;
        let unit = unit.trim();
        let unit = if unit.is_empty() {
            Unit::Dimensionless
        } else {
            Unit::parse(unit).ok_or_else(|| format!("unknown unit '{unit}'"))?
        };
        Ok(Quantity::new(value, unit))
    }
}

impl fmt::Display for Quantity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} mm", self.mm)
    }
}
