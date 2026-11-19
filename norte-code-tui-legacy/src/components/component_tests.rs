use super::ComponentId;

#[test]
fn every_tab_has_its_own_digit() {
    for tab in ComponentId::tabs() {
        let digit = tab.shortcut().expect("a tab carries a digit");
        assert_eq!(ComponentId::from_shortcut(digit), Some(tab));
    }
}

#[test]
fn the_tabs_are_numbered_in_display_order() {
    let digits = ComponentId::tabs()
        .iter()
        .filter_map(|tab| tab.shortcut())
        .collect::<String>();
    assert_eq!(digits, "123456789");
}

#[test]
fn the_review_is_not_a_tab() {
    assert_eq!(ComponentId::PlanReview.shortcut(), None);
    assert_eq!(ComponentId::from_shortcut('0'), None);
}

#[test]
fn a_letter_selects_no_tab() {
    assert_eq!(ComponentId::from_shortcut('c'), None);
    assert_eq!(ComponentId::from_shortcut('s'), None);
    assert_eq!(ComponentId::from_shortcut('g'), None);
}

#[test]
fn tab_navigation_wraps_at_both_ends() {
    assert_eq!(ComponentId::Costs.next_tab(), ComponentId::Session);
    assert_eq!(ComponentId::Session.previous_tab(), ComponentId::Costs);
}
