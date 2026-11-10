use super::*;

fn menu(labels: &[&str]) -> Menu {
    Menu::new(
        labels
            .iter()
            .map(|label| Item::new(*label, false))
            .collect(),
    )
}

fn rows() -> Vec<Item> {
    vec![
        Item::new("New session", true),
        Item::new("Last session", true).noted("unfinished"),
        Item::new("55fd07f3", false).searched_by("fix the footer"),
        Item::new("69be4643", false).searched_by("write the guide"),
    ]
}

#[test]
fn the_cursor_walks_the_list_and_holds_at_both_ends() {
    let mut menu = menu(&["new", "last", "one", "two"]);
    assert_eq!(menu.selected(), Some(0));
    assert_eq!(menu.key(Press::Down), Step::Continue);
    assert_eq!(menu.selected(), Some(1));
    assert_eq!(menu.key(Press::Down), Step::Continue);
    assert_eq!(menu.selected(), Some(2));
    assert_eq!(menu.key(Press::Up), Step::Continue);
    assert_eq!(menu.selected(), Some(1));
    // The ends hold instead of wrapping: a wrap is how a person opens the
    // session at the other end of the list without ever seeing it.
    menu.key(Press::Home);
    menu.key(Press::Up);
    menu.key(Press::Up);
    assert_eq!(menu.selected(), Some(0), "up at the top stays at the top");
    menu.key(Press::End);
    assert_eq!(menu.selected(), Some(3));
    menu.key(Press::Down);
    assert_eq!(menu.selected(), Some(3), "down at the bottom stays");
}

#[test]
fn enter_takes_the_row_under_the_cursor() {
    let mut menu = menu(&["new", "last", "one"]);
    menu.key(Press::Down);
    menu.key(Press::Down);
    assert_eq!(menu.key(Press::Enter), Step::Accept);
    assert_eq!(menu.selected(), Some(2));
    let item = menu.selected_item().expect("a row");
    assert_eq!(item.label, "one");
}

#[test]
fn escape_and_control_c_leave_without_choosing() {
    let mut menu = menu(&["new", "last"]);
    assert_eq!(menu.key(Press::Escape), Step::Cancel);
    assert_eq!(menu.key(Press::Interrupt), Step::Cancel);
    // Leaving changes nothing: a caller that treats it as a default gets the
    // menu it already had.
    assert_eq!(menu.selected(), Some(0));
}

#[test]
fn typing_narrows_the_list_and_backspace_widens_it_again() {
    let mut menu = Menu::new(rows());
    assert_eq!(menu.len(), 4);
    for character in "guide".chars() {
        assert_eq!(menu.key(Press::Char(character)), Step::Continue);
    }
    assert_eq!(menu.len(), 1, "only the guide session survives");
    assert_eq!(
        menu.selected_item().map(|item| item.label.as_str()),
        Some("69be4643")
    );
    menu.key(Press::Backspace);
    assert_eq!(menu.filter(), "guid");
    menu.key(Press::Clear);
    assert_eq!(menu.len(), 4, "clearing puts the whole list back");
}

#[test]
fn the_filter_ignores_case_and_reads_what_the_row_carries() {
    let mut menu = Menu::new(rows());
    for character in "FOOTER".chars() {
        menu.push(character);
    }
    assert_eq!(menu.len(), 1, "the filter is case-insensitive");
    assert_eq!(
        menu.selected_item().map(|item| item.label.as_str()),
        Some("55fd07f3")
    );
    // The note is drawn on the row, so it filters too.
    menu.clear_filter();
    for character in "unfinished".chars() {
        menu.push(character);
    }
    assert_eq!(
        menu.selected_item().map(|item| item.label.as_str()),
        Some("Last session")
    );
    // And the key is in the label.
    menu.clear_filter();
    for character in "69be".chars() {
        menu.push(character);
    }
    assert_eq!(
        menu.selected_item().map(|item| item.label.as_str()),
        Some("69be4643")
    );
}

#[test]
fn the_cursor_stays_on_a_row_when_the_filter_shrinks_the_list() {
    // The cursor is an index into the filtered list, so a filter that cuts
    // four rows down to one must not leave Enter choosing nothing.
    let mut menu = Menu::new(rows());
    menu.key(Press::End);
    assert_eq!(menu.selected(), Some(3));
    for character in "fix".chars() {
        menu.push(character);
    }
    assert_eq!(menu.len(), 1);
    assert_eq!(
        menu.selected(),
        Some(2),
        "the cursor moved onto the survivor"
    );
    // And a filter that keeps nothing leaves nothing to choose, which the
    // caller reports rather than acting on.
    menu.push('z');
    assert!(menu.is_empty());
    assert_eq!(menu.selected(), None);
    assert_eq!(menu.key(Press::Enter), Step::Accept);
    assert_eq!(menu.selected(), None);
}

#[test]
fn backspace_on_an_empty_filter_does_not_leave_the_menu() {
    // It is the key a person presses to fix a typo. Turning it into a cancel
    // left the menu on the keystroke meant to keep it open.
    let mut menu = menu(&["new", "last"]);
    assert_eq!(menu.key(Press::Backspace), Step::Continue);
    assert_eq!(menu.filter(), "");
    assert_eq!(menu.len(), 2);
    assert_eq!(menu.key(Press::Enter), Step::Accept);
}

#[test]
fn page_keys_and_the_ends_reach_the_rest_of_a_long_list() {
    let labels: Vec<String> = (0..40).map(|index| format!("session {index:02}")).collect();
    let borrowed: Vec<&str> = labels.iter().map(String::as_str).collect();
    let mut menu = menu(&borrowed);
    menu.key(Press::PageDown);
    assert_eq!(menu.selected(), Some(VISIBLE_ROWS));
    menu.key(Press::PageUp);
    assert_eq!(menu.selected(), Some(0));
    menu.key(Press::End);
    assert_eq!(menu.selected(), Some(39));
    menu.key(Press::PageDown);
    assert_eq!(menu.selected(), Some(39), "the end holds");
    menu.key(Press::Home);
    assert_eq!(menu.selected(), Some(0));
}

#[test]
fn the_window_follows_the_cursor_and_counts_what_it_left_out() {
    let labels: Vec<String> = (0..40).map(|index| format!("model {index:02}")).collect();
    let borrowed: Vec<&str> = labels.iter().map(String::as_str).collect();
    let mut menu = menu(&borrowed);
    let (window, above, below) = menu.window();
    assert_eq!(window, 0..VISIBLE_ROWS, "a full first screen");
    assert_eq!(above, 0, "nothing above the cursor at the top");
    assert_eq!(below, 40 - VISIBLE_ROWS, "the rest is counted");
    menu.key(Press::End);
    let (window, above, below) = menu.window();
    assert_eq!(window.end, 40, "the last row is in view");
    assert_eq!(below, 0);
    assert_eq!(above, 40 - VISIBLE_ROWS);
    // A list that fits is drawn whole and hides nothing.
    let (window, above, below) =
        Menu::new(vec![Item::new("a", false), Item::new("b", false)]).window();
    assert_eq!(window.len(), 2);
    assert_eq!((above, below), (0, 0));
}

#[test]
fn stripping_style_leaves_the_text_and_drops_the_codes() {
    let styled = format!("{}\x1b[0m {}", Style::plain().cyan("hex"), "value");
    assert_eq!(strip_style(&styled), "hex value");
    assert_eq!(strip_style("plain"), "plain");
}

#[test]
fn enter_on_an_empty_filter_hands_back_what_was_typed() {
    // The menus were numbered prompts that took a typed name, and the name
    // of something the list does not hold is exactly what a filter that
    // matched nothing is. It is passed through rather than dropped.
    let mut menu = Menu::new(rows());
    for character in "river/deepseek".chars() {
        menu.push(character);
    }
    assert!(menu.is_empty());
    assert_eq!(chosen(&menu), Outcome::Typed("river/deepseek".to_owned()));
    // With a row under the cursor it is the row that wins.
    let mut menu = Menu::new(rows());
    menu.key(Press::Down);
    assert_eq!(chosen(&menu), Outcome::Chosen(1));
}

#[test]
fn the_hint_says_what_the_filter_is() {
    assert!(hint("").contains("type to filter"), "{}", hint(""));
    assert!(hint("riv").contains("filter `riv`"), "{}", hint("riv"));
}
