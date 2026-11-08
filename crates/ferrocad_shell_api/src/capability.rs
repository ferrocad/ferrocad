//! What a host grants a script, as opposed to what the script asks for.
//!
//! A mod's manifest says what it *wants* (`document: read-write`); this is what the host
//! that booted it can actually give. They are not always the same, and the gap is the
//! point: a Python mod that edits on the desktop cannot edit in the browser, because the
//! browser's Python reaches the document through a single read function. A host that cannot
//! grant what was asked has to say so, or the first refused write is the only warning the
//! author ever gets.

use std::fmt;

/// What a script half may do to the open document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DocumentGrant {
    /// The tree and the property rows; writes are refused.
    Read,
    /// Read and write: create, set properties, recompute.
    ReadWrite,
}

impl DocumentGrant {
    /// Both grants, weakest first.
    pub const ALL: [DocumentGrant; 2] = [DocumentGrant::Read, DocumentGrant::ReadWrite];

    /// The name a manifest and the host log use.
    pub fn id(self) -> &'static str {
        match self {
            DocumentGrant::Read => "read",
            DocumentGrant::ReadWrite => "read-write",
        }
    }

    /// The grant a manifest's word asks for.
    ///
    /// `read-only`, `readonly` and `write` are accepted beside the canonical pair because
    /// the wasm manifests and the page loader already spell them that way.
    pub fn from_name(name: &str) -> Option<Self> {
        match name.trim() {
            "read" | "read-only" | "readonly" => Some(DocumentGrant::Read),
            "read-write" | "readwrite" | "write" => Some(DocumentGrant::ReadWrite),
            _ => None,
        }
    }

    /// Whether this grant lets a script change the document.
    pub fn allows_write(self) -> bool {
        matches!(self, DocumentGrant::ReadWrite)
    }
}

impl fmt::Display for DocumentGrant {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.id())
    }
}
