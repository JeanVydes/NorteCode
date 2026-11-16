use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

fn temporary(name: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "norte-code-keys-{}-{}-{name}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

#[test]
fn a_present_variable_wins_over_an_absent_one() {
    // The environment is read, not mutated: other tests share this process.
    let Ok(path) = std::env::var("PATH") else {
        eprintln!("skipping: PATH is not set in this environment");
        return;
    };
    let key = environment_key(
        "p",
        &["NORTE_CODE_TEST_ABSENT_KEY".to_owned(), "PATH".to_owned()],
    )
    .expect("key");
    assert_eq!(key.expose(), path.trim());
}

#[test]
fn a_provider_with_no_variable_needs_no_key() {
    let key = environment_key("p", &[]).expect("key");
    assert_eq!(key.expose(), "");
}

#[test]
fn a_missing_key_names_what_was_tried() {
    let error = environment_key(
        "ollamacloud",
        &[
            "NORTE_CODE_TEST_MISSING_A".to_owned(),
            "NORTE_CODE_TEST_MISSING_B".to_owned(),
        ],
    )
    .expect_err("no key");
    let message = error.to_string();
    assert!(message.contains("ollamacloud"), "{message}");
    assert!(message.contains("NORTE_CODE_TEST_MISSING_A"), "{message}");
    assert!(message.contains("NORTE_CODE_TEST_MISSING_B"), "{message}");
}

#[test]
fn a_saved_key_round_trips_and_keeps_its_neighbours() {
    let path = temporary("secrets.json");
    save_secret(&path, "one", " first ").expect("save one");
    save_secret(&path, "two", "second").expect("save two");
    assert_eq!(read_secret(&path, "one").expect("one").expose(), "first");
    assert_eq!(read_secret(&path, "two").expect("two").expose(), "second");
    assert!(read_secret(&path, "three").is_none());
    let _ = fs::remove_file(path);
}

#[test]
fn a_corrupt_file_reads_as_empty_and_is_replaced() {
    let path = temporary("corrupt.json");
    fs::write(&path, "{not json").expect("write");
    assert!(read_secret(&path, "one").is_none());
    save_secret(&path, "one", "key").expect("save");
    assert_eq!(read_secret(&path, "one").expect("one").expose(), "key");
    let _ = fs::remove_file(path);
}

#[cfg(unix)]
#[test]
fn a_saved_key_is_private_even_over_a_loose_file() {
    use std::os::unix::fs::PermissionsExt;
    let path = temporary("private.json");
    fs::write(&path, "{}").expect("write");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("mode");
    save_secret(&path, "one", "key").expect("save");
    let mode = fs::metadata(&path).expect("metadata").permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "the file holds a key");
    let _ = fs::remove_file(path);
}
