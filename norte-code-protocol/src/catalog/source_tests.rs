use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn temp_dir() -> PathBuf {
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("norte-code-catalogue-{}-{n}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    dir
}

#[test]
fn the_two_files_sit_beside_the_configuration() {
    // The paths are derived, and a test that set the variables would race
    // every other test in the process: what is asserted is the shape, that
    // both files are under one directory and named as documented.
    let dir = catalog_dir().expect("a directory");
    assert_eq!(providers_path().expect("path"), dir.join("providers.json"));
    assert_eq!(models_path().expect("path"), dir.join("models.json"));
    assert!(dir.ends_with("norte-code"), "{}", dir.display());
}

#[test]
fn a_catalogue_that_is_there_is_never_written_over() {
    // What a person wrote is theirs, and a file emptied on purpose stays
    // empty: the second call writes nothing and changes nothing.
    let dir = temp_dir();
    ensure_in(&dir).expect("written");
    std::fs::write(dir.join("providers.json"), "{\"providers\":{}}").expect("write");
    let created = ensure_in(&dir).expect("checked");
    assert!(!created.providers && !created.models, "{created:?}");
    assert!(!created.any());
}
