//! Typed parse errors with a source position.
//!
//! Every error names the file, a 1-based line and column, and what the parser
//! expected. Structural errors also report the byte offset of the offending
//! element (XML syntax errors come from `roxmltree` and only carry a row/column).

use std::fmt;

/// A `.ui` parse failure.
///
/// `line` and `column` are 1-based. `offset` is the byte offset of the start of
/// the offending element when one is available (`None` for I/O and XML syntax
/// errors).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiError {
    /// Display name of the form: a filesystem path, or `<memory>` for string input.
    pub file: String,
    /// 1-based line number.
    pub line: u32,
    /// 1-based column, counted in Unicode scalar values.
    pub column: u32,
    /// Byte offset of the offending element, when known.
    pub offset: Option<usize>,
    /// The kind of failure, including the expected construct.
    pub kind: UiErrorKind,
}

/// The reason a form could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiErrorKind {
    /// The file could not be read. Position is always `1:1`.
    Io { message: String },
    /// The document is not well-formed XML.
    Xml { message: String },
    /// An element that is not allowed in this position. `parent` describes the
    /// containing element, `expected` lists what the parser accepts there.
    UnexpectedElement {
        parent: String,
        found: String,
        expected: String,
    },
    /// A required child element is missing.
    MissingElement { parent: String, expected: String },
    /// A required attribute is missing.
    MissingAttribute { element: String, attribute: String },
    /// An attribute value has the wrong shape.
    InvalidAttribute {
        element: String,
        attribute: String,
        value: String,
        expected: String,
    },
    /// An element's text has the wrong shape.
    InvalidValue {
        element: String,
        value: String,
        expected: String,
    },
    /// A `<property>` value element this reader does not model.
    UnsupportedValue { element: String },
    /// A widget or layout class outside the recognised set.
    UnknownClass { class: String, expected: String },
    /// A construct that may occur at most once occurred twice.
    Duplicate { element: String, parent: String },
    /// The element nesting exceeded [`crate::MAX_DEPTH`].
    NestingTooDeep { limit: usize },
}

impl UiError {
    pub(crate) fn at_line_col(
        file: &str,
        line: u32,
        column: u32,
        offset: Option<usize>,
        kind: UiErrorKind,
    ) -> Self {
        Self {
            file: file.to_owned(),
            line,
            column,
            offset,
            kind,
        }
    }

    /// Resolve a byte offset in `source` to a 1-based line and column.
    ///
    /// Offsets that are not on a UTF-8 boundary are rounded down, so this never
    /// panics on odd input.
    pub(crate) fn at_offset(file: &str, source: &str, offset: usize, kind: UiErrorKind) -> Self {
        let offset = clamp_offset(source, offset);
        let (line, column) = line_col(source, offset);
        Self {
            file: file.to_owned(),
            line,
            column,
            offset: Some(offset),
            kind,
        }
    }

    /// A human-readable description of the failure, without the position prefix.
    pub fn message(&self) -> String {
        match &self.kind {
            UiErrorKind::Io { message } => format!("cannot read form: {message}"),
            UiErrorKind::Xml { message } => format!("malformed XML: {message}"),
            UiErrorKind::UnexpectedElement {
                parent,
                found,
                expected,
            } => format!("expected {expected} in {parent}, found <{found}>"),
            UiErrorKind::MissingElement { parent, expected } => {
                format!("expected {expected} in {parent}")
            }
            UiErrorKind::MissingAttribute { element, attribute } => {
                format!("<{element}> is missing the required '{attribute}' attribute")
            }
            UiErrorKind::InvalidAttribute {
                element,
                attribute,
                value,
                expected,
            } => format!("<{element}> attribute '{attribute}' = '{value}' is not {expected}"),
            UiErrorKind::InvalidValue {
                element,
                value,
                expected,
            } => format!("<{element}> value '{value}' is not {expected}"),
            UiErrorKind::UnsupportedValue { element } => {
                format!("unsupported property value element <{element}>")
            }
            UiErrorKind::UnknownClass { class, expected } => {
                format!("unknown class '{class}': expected {expected}")
            }
            UiErrorKind::Duplicate { element, parent } => {
                format!("duplicate <{element}> in {parent}")
            }
            UiErrorKind::NestingTooDeep { limit } => {
                format!("element nesting exceeds the limit of {limit}")
            }
        }
    }
}

impl fmt::Display for UiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}:{}: {}",
            self.file,
            self.line,
            self.column,
            self.message()
        )
    }
}

impl std::error::Error for UiError {}

fn clamp_offset(source: &str, offset: usize) -> usize {
    let mut offset = offset.min(source.len());
    while offset > 0 && !source.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

fn line_col(source: &str, offset: usize) -> (u32, u32) {
    let mut line = 1u32;
    let mut column = 1u32;
    for ch in source[..offset].chars() {
        if ch == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    (line, column)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_col_is_one_based_and_handles_multibyte_text() {
        let source = "a\né\nb";
        assert_eq!(line_col(source, 0), (1, 1));
        assert_eq!(line_col(source, 1), (1, 2));
        assert_eq!(line_col(source, 2), (2, 1));
        // "é" is two bytes; offset 3 is the second byte and must round down.
        assert_eq!(line_col(source, clamp_offset(source, 3)), (2, 1));
        assert_eq!(line_col(source, clamp_offset(source, 4)), (2, 2));
    }
}
