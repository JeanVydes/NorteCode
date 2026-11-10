use super::ToolEnvironment;

#[test]
fn a_note_names_the_home_and_the_display() {
    let environment = ToolEnvironment {
        variables: [("HOME".to_owned(), "/home/someone".to_owned())]
            .into_iter()
            .collect(),
        display: Some("wayland:wayland-0".to_owned()),
    };
    let note = environment.note();
    assert!(note.contains("`/home/someone`"), "{note}");
    assert!(note.contains("`wayland:wayland-0`"), "{note}");
}

#[test]
fn a_host_without_a_display_says_so() {
    let environment = ToolEnvironment::default();
    let note = environment.note();
    assert!(note.contains("no graphical session"), "{note}");
    assert!(environment.home().is_none());
}
