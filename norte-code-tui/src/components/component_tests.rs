use super::ComponentId;

#[test]
fn costs_view_uses_a_letter_shortcut_outside_session_number_keys() {
    assert_eq!(ComponentId::from_shortcut('c'), Some(ComponentId::Costs));
    assert_eq!(ComponentId::from_shortcut('8'), None);
}
