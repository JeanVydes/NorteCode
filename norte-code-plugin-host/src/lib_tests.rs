use super::{PLUGIN_ABI_VERSION, validate_abi};

#[test]
fn accepts_plugins_built_for_the_current_interface() {
    assert!(validate_abi(PLUGIN_ABI_VERSION).is_ok());
}

#[test]
fn rejects_plugins_built_for_an_older_interface_before_loading_them() {
    let error = validate_abi(PLUGIN_ABI_VERSION - 1).expect_err("older ABI must be rejected");
    assert!(error.contains("plugin interface 1"));
    assert!(error.contains("host speaks 2"));
}
