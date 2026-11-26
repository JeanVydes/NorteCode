use super::{enter_screen, leave_screen};

#[test]
fn keyboard_protocol_is_enabled_and_disabled_on_the_alternate_screen() {
    let mut bytes = Vec::new();
    enter_screen(&mut bytes).unwrap();
    let enter = String::from_utf8(bytes).unwrap();
    let alternate = enter.find("\x1b[?1049h").unwrap();
    let keyboard = enter.find("\x1b[>11u").unwrap();
    assert!(
        alternate < keyboard,
        "keyboard flags must be pushed after switching screens"
    );

    let mut bytes = Vec::new();
    leave_screen(&mut bytes).unwrap();
    let leave = String::from_utf8(bytes).unwrap();
    let keyboard = leave.find("\x1b[<1u").unwrap();
    let alternate = leave.find("\x1b[?1049l").unwrap();
    assert!(
        keyboard < alternate,
        "keyboard flags must be popped before restoring the main screen"
    );
}
