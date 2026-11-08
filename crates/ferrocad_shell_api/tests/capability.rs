//! The grant vocabulary: what a host can give, and which manifest words ask for it.

use ferrocad_shell_api::DocumentGrant;

#[test]
fn the_names_round_trip() {
    for grant in DocumentGrant::ALL {
        assert_eq!(DocumentGrant::from_name(grant.id()), Some(grant));
        assert_eq!(grant.to_string(), grant.id());
    }
}

#[test]
fn the_spellings_a_manifest_already_uses_are_accepted() {
    // The wasm manifests and the page loader wrote these before the seam had the type.
    assert_eq!(
        DocumentGrant::from_name("read-only"),
        Some(DocumentGrant::Read)
    );
    assert_eq!(DocumentGrant::from_name("readonly"), Some(DocumentGrant::Read));
    assert_eq!(
        DocumentGrant::from_name("write"),
        Some(DocumentGrant::ReadWrite)
    );
    assert_eq!(
        DocumentGrant::from_name(" read-write "),
        Some(DocumentGrant::ReadWrite)
    );
    // Not a grant: an unknown word is refused rather than rounded to a default.
    assert_eq!(DocumentGrant::from_name("rw"), None);
    assert_eq!(DocumentGrant::from_name(""), None);
}

#[test]
fn only_one_grant_writes() {
    assert!(!DocumentGrant::Read.allows_write());
    assert!(DocumentGrant::ReadWrite.allows_write());
}
