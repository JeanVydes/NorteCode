use super::{check, flag};

#[test]
fn a_failed_smoke_check_is_an_error() {
    assert_eq!(check(false, "broken".to_owned()), Err("broken".to_owned()));
}

#[test]
fn a_flag_without_a_value_is_absent() {
    assert_eq!(flag(&["--prompt".to_owned()], "--prompt"), None);
}
