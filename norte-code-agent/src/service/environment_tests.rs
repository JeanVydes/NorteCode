use super::ToolEnvironment;

#[test]
fn a_note_names_the_home() {
    let environment = ToolEnvironment {
        variables: [("HOME".to_owned(), "/home/someone".to_owned())]
            .into_iter()
            .collect(),
    };
    let note = environment.note();
    assert!(note.contains("`/home/someone`"), "{note}");
    assert!(note.contains("`PATH` is inherited"), "{note}");
}

#[test]
fn a_host_without_a_home_says_so() {
    let environment = ToolEnvironment::default();
    let note = environment.note();
    assert!(note.contains("`HOME` is not set"), "{note}");
    assert!(environment.home().is_none());
}
