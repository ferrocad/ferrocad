//! A minimal interned-string hasher (FreeCAD `StringHasher` / `StringID`).
//!
//! `StringHasher` interns strings into a shared table and returns `StringId`
//! handles; `StringId` carries a reference back to its table so `is_same`
//! compares both the table identity and the integer value.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// A shared interned-string table. Values are 1-based; index 0 is a sentinel
/// so that "value 0" means "not found" (matching FreeCAD's `getID(int)`).
struct Table {
    strings: Vec<String>,
    index: HashMap<String, usize>,
}

impl Default for Table {
    fn default() -> Self {
        Self {
            strings: vec![String::new()], // sentinel at value 0
            index: HashMap::new(),
        }
    }
}

impl Table {
    fn intern(&mut self, text: &str) -> usize {
        if let Some(&v) = self.index.get(text) {
            return v;
        }
        let v = self.strings.len();
        self.strings.push(text.to_string());
        self.index.insert(text.to_string(), v);
        v
    }

    fn get(&self, v: usize) -> Option<&str> {
        if v >= 1 && v < self.strings.len() {
            Some(&self.strings[v])
        } else {
            None
        }
    }
}

#[derive(Clone)]
pub struct StringHasher {
    table: Arc<Mutex<Table>>,
}

impl Default for StringHasher {
    fn default() -> Self {
        Self {
            table: Arc::new(Mutex::new(Table::default())),
        }
    }
}

impl StringHasher {
    pub fn new() -> Self {
        Self::default()
    }

    /// Intern `text`, returning a handle unique within this hasher.
    pub fn get_id(&self, text: &str) -> StringId {
        let value = self.table.lock().unwrap().intern(text);
        StringId {
            hasher: self.table.clone(),
            value,
        }
    }

    /// Look up a previously-interned id by its integer value.
    pub fn find_id(&self, value: usize) -> Option<StringId> {
        let table = self.table.lock().unwrap();
        table
            .get(value)
            .map(|_| StringId {
                hasher: self.table.clone(),
                value,
            })
    }

    pub fn len(&self) -> usize {
        // Exclude the value-0 sentinel.
        self.table.lock().unwrap().strings.len() - 1
    }

    pub fn is_same(&self, other: &StringHasher) -> bool {
        Arc::ptr_eq(&self.table, &other.table)
    }
}

#[derive(Clone)]
pub struct StringId {
    hasher: Arc<Mutex<Table>>,
    value: usize,
}

impl StringId {
    pub fn value(&self) -> usize {
        self.value
    }

    pub fn data(&self) -> String {
        self.hasher
            .lock()
            .unwrap()
            .get(self.value)
            .unwrap_or("")
            .to_string()
    }

    pub fn is_same(&self, other: &StringId) -> bool {
        Arc::ptr_eq(&self.hasher, &other.hasher) && self.value == other.value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interns_and_looks_up() {
        let h = StringHasher::new();
        let a = h.get_id("A");
        let b = h.get_id("A");
        assert_eq!(a.value(), b.value());
        assert!(a.is_same(&b));
        assert_eq!(a.data(), "A");
        assert_eq!(h.len(), 1);
    }

    #[test]
    fn distinct_strings_get_distinct_ids() {
        let h = StringHasher::new();
        let a = h.get_id("A");
        let b = h.get_id("B");
        assert_ne!(a.value(), b.value());
        assert!(!a.is_same(&b));
    }

    #[test]
    fn hashers_are_not_same() {
        let h1 = StringHasher::new();
        let h2 = StringHasher::new();
        assert!(!h1.is_same(&h2));
        assert!(h1.is_same(&h1));
    }
}
