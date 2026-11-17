use std::path::Path;

use super::display_path_under;

#[test]
fn a_path_under_home_is_shortened_to_a_tilde() {
    let path = Path::new("/home/jean/pictures/a.png");

    assert_eq!(
        display_path_under(path, Some(Path::new("/home/jean"))),
        "~/pictures/a.png"
    );
}

#[test]
fn a_path_outside_home_is_shown_whole() {
    let path = Path::new("/srv/shared/a.png");

    assert_eq!(
        display_path_under(path, Some(Path::new("/home/jean"))),
        "/srv/shared/a.png"
    );
}
