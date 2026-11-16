use super::*;

fn manifest() -> BundleManifest {
    BundleManifest {
        bundle_version: "1".to_owned(),
        raep_document_version: Some("0.1.0".to_owned()),
        wire_version: Some("1.0".to_owned()),
        exported_at: 1,
        exporter: None,
        root_session_id: "source".to_owned(),
        sessions: vec![BundleSession {
            session_id: "source".to_owned(),
            parent_session_id: None,
            agent: serde_json::json!({"id": "test"}),
            state: None,
            history_file: "sessions/source/history.jsonl".to_owned(),
            last_seq: None,
            usage_total: None,
        }],
        files: Vec::new(),
        excluded: Vec::new(),
    }
}

#[test]
fn bundle_round_trip_checks_manifest_hashes() {
    let files = BTreeMap::from([(
        "sessions/source/history.jsonl".to_owned(),
        b"{\"role\":\"user\",\"content\":\"hi\"}\n".to_vec(),
    )]);
    let bytes = create_bundle(manifest(), files.clone()).expect("bundle");
    let (manifest, restored) = read_bundle(&bytes, 1024).expect("read bundle");
    assert_eq!(manifest.files.len(), 1);
    assert_eq!(restored, files);
}

#[test]
fn bundle_rejects_parent_paths() {
    let files = BTreeMap::from([("../outside".to_owned(), b"no".to_vec())]);
    assert!(matches!(
        create_bundle(manifest(), files),
        Err(BundleError::Invalid(_))
    ));
}

#[test]
fn bundle_rejects_disconnected_sessions() {
    let mut manifest = manifest();
    manifest.sessions.push(BundleSession {
        session_id: "child".to_owned(),
        parent_session_id: Some("missing".to_owned()),
        agent: serde_json::json!({"id": "test"}),
        state: None,
        history_file: "sessions/child/history.jsonl".to_owned(),
        last_seq: None,
        usage_total: None,
    });
    let files = BTreeMap::from([
        (
            "sessions/source/history.jsonl".to_owned(),
            b"{\"role\":\"user\",\"content\":\"hi\"}\n".to_vec(),
        ),
        ("sessions/child/history.jsonl".to_owned(), Vec::new()),
    ]);
    assert!(matches!(
        create_bundle(manifest, files),
        Err(BundleError::Invalid(_))
    ));
}

#[test]
fn bundle_rejects_hash_mismatches() {
    let mut manifest = manifest();
    manifest.files.push(BundleFile {
        path: "sessions/source/history.jsonl".to_owned(),
        sha256: "0".repeat(64),
        bytes: 2,
    });
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .start_file("manifest.json", SimpleFileOptions::default())
        .expect("manifest entry");
    writer
        .write_all(&serde_json::to_vec(&manifest).expect("manifest JSON"))
        .expect("manifest bytes");
    writer
        .start_file(
            "sessions/source/history.jsonl",
            SimpleFileOptions::default(),
        )
        .expect("history entry");
    writer.write_all(b"{}").expect("history bytes");
    let bytes = writer.finish().expect("finish bundle").into_inner();
    assert!(matches!(
        read_bundle(&bytes, 1024),
        Err(BundleError::Invalid(_))
    ));
}
