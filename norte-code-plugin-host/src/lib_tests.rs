use super::{PLUGIN_ABI_VERSION, validate_abi};

#[test]
fn accepts_plugins_built_for_the_current_interface() {
    assert!(validate_abi(PLUGIN_ABI_VERSION).is_ok());
}

#[test]
fn rejects_plugins_built_for_an_older_interface_before_loading_them() {
    let older = PLUGIN_ABI_VERSION - 1;
    let error = validate_abi(older).expect_err("older ABI must be rejected");
    assert!(error.contains(&format!("plugin interface {older}")));
    assert!(error.contains(&format!("host speaks {PLUGIN_ABI_VERSION}")));
}
