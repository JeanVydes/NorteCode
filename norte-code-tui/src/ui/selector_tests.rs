use super::Selector;

#[test]
fn an_empty_selector_has_no_value() {
    let selector = Selector::new("queue", vec![], 0);

    assert_eq!(selector.value(), None);
}
