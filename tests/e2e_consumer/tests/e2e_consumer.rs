//! Freeze-1 placeholder: proves the crate builds and links `attest_core`.

#[test]
fn skeleton_links_attest_core() {
    assert_eq!(attest_core::EsaError::E11AlreadyConsumed.name(), "E11_ALREADY_CONSUMED");
}
