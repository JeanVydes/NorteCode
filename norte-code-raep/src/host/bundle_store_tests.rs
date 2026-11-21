//! Bundle spools keep large payloads out of the host's heap.

use serde::{Deserialize, Serialize};

use super::{prepare_dir, read_bytes, read_json, remove, write_bytes, write_json};

#[derive(Debug, Deserialize, PartialEq, Serialize)]
struct Preview {
    title: String,
}

#[test]
fn binary_bundles_round_trip_from_disk() {
    let dir = std::env::temp_dir().join(format!("norte-spool-test-{}", std::process::id()));
    let path = write_bytes(&dir, b"bundle bytes").expect("spool");

    assert_eq!(read_bytes(&path).expect("read"), b"bundle bytes");

    remove(&path);
    assert!(!path.exists());
    let _ = std::fs::remove_dir(&dir);
}

#[test]
fn json_previews_round_trip_from_disk() {
    let dir = std::env::temp_dir().join(format!("norte-spool-json-test-{}", std::process::id()));
    prepare_dir(&dir).expect("dir");
    let path = write_json(
        &dir,
        &Preview {
            title: "a session".to_owned(),
        },
    )
    .expect("spool");

    assert_eq!(
        read_json::<Preview>(&path).expect("read"),
        Preview {
            title: "a session".to_owned()
        }
    );

    remove(&path);
    let _ = std::fs::remove_dir(&dir);
}
