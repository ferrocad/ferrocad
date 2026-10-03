//! Quantity parsing and conversion.

use crate::Quantity;

#[test]
fn quantity_parses_and_converts() {
    let q: Quantity = "10 mm".parse().unwrap();
    assert_eq!(q.value_mm(), 10.0);

    let q: Quantity = "2.5 in".parse().unwrap();
    assert!((q.value_mm() - 63.5).abs() < 1e-9);

    let q: Quantity = "1 m".parse().unwrap();
    assert_eq!(q.value_mm(), 1000.0);

    assert!("10 bogus".parse::<Quantity>().is_err());
}
