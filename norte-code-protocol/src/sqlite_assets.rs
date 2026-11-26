//! Storage shared by the session graph and RAEP's file index.

use std::path::Path;

use rusqlite::{Connection, Transaction, params};

use super::Fault;

pub(super) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS raep_uploads (
    upload_id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    owner TEXT,
    blob_id TEXT NOT NULL,
    mime_type TEXT NOT NULL,
    bytes INTEGER NOT NULL,
    sha256 TEXT NOT NULL,
    expires_at INTEGER NOT NULL,
    referenced INTEGER NOT NULL DEFAULT 0,
    details TEXT
);
CREATE INDEX IF NOT EXISTS idx_raep_uploads_session ON raep_uploads(session_id, upload_id);
CREATE INDEX IF NOT EXISTS idx_raep_uploads_blob ON raep_uploads(blob_id);
CREATE TABLE IF NOT EXISTS raep_global_artifacts (
    blob_id TEXT NOT NULL,
    owner TEXT NOT NULL,
    PRIMARY KEY (blob_id, owner)
);
CREATE TABLE IF NOT EXISTS raep_asset_migration (id INTEGER PRIMARY KEY);
CREATE TABLE IF NOT EXISTS raep_session_owners (
    session_id TEXT PRIMARY KEY,
    owner TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS session_node_owners (
    session_id TEXT NOT NULL,
    node_id TEXT NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    PRIMARY KEY (session_id, node_id)
);
CREATE TABLE IF NOT EXISTS session_compactions (
    session_id TEXT PRIMARY KEY,
    previous_session_id TEXT NOT NULL
);
";

pub(super) fn migrate(connection: &Connection, old: &Path) -> rusqlite::Result<()> {
    if !old.is_file()
        || connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM raep_asset_migration WHERE id = 1)",
            [],
            |row| row.get::<_, bool>(0),
        )?
    {
        return Ok(());
    }
    connection.execute("ATTACH DATABASE ?1 AS old_uploads", [old.to_string_lossy()])?;
    let result = (|| {
        let tx = connection.unchecked_transaction()?;
        tx.execute_batch(
            "INSERT OR IGNORE INTO raep_uploads
             (upload_id, session_id, owner, blob_id, mime_type, bytes, sha256, expires_at, referenced)
             SELECT upload_id, session_id, owner, blob_id, mime_type, bytes, sha256, expires_at, referenced
             FROM old_uploads.raep_uploads;
             INSERT OR IGNORE INTO raep_session_owners
             SELECT session_id, min(owner) FROM old_uploads.raep_uploads
             WHERE owner IS NOT NULL
             GROUP BY session_id HAVING count(DISTINCT owner) = 1;
             INSERT OR IGNORE INTO raep_asset_migration VALUES (1);",
        )?;
        tx.commit()
    })();
    let detach = connection.execute_batch("DETACH DATABASE old_uploads");
    result.and(detach)
}

pub(super) fn delete_session(tx: &Transaction<'_>, key: &str) -> Result<(), Fault> {
    tx.execute_batch(
        "CREATE TEMP TABLE removed_sessions (id TEXT PRIMARY KEY);
         CREATE TEMP TABLE removed_nodes (id TEXT PRIMARY KEY);
         CREATE TEMP TABLE removed_blobs (id TEXT PRIMARY KEY);",
    )?;
    tx.execute(
        "INSERT OR IGNORE INTO removed_sessions
         SELECT north FROM logical_sessions WHERE key = ?1
         UNION SELECT json_extract(data, '$.compacted_from') FROM logical_sessions WHERE key = ?1
         UNION SELECT json_extract(boundary.value, '$.session')
               FROM logical_sessions, json_each(stats, '$.turn_boundaries') AS boundary
               WHERE logical_sessions.key = ?1",
        params![key],
    )?;
    tx.execute(
        "INSERT OR IGNORE INTO removed_blobs SELECT blob_id FROM pending_steer_blobs
         WHERE session_id = ?1 AND blob_id IS NOT NULL",
        [key],
    )?;
    tx.execute("DELETE FROM logical_sessions WHERE key = ?1", [key])?;
    tx.execute(
        "DELETE FROM raep_session_owners WHERE session_id = ?1",
        [key],
    )?;
    tx.execute_batch(
        "WITH RECURSIVE earlier(id) AS (
             SELECT id FROM removed_sessions WHERE id IS NOT NULL
             UNION SELECT previous_session_id FROM session_compactions JOIN earlier
                   ON session_compactions.session_id = earlier.id
         )
         INSERT OR IGNORE INTO removed_sessions SELECT id FROM earlier;
         DELETE FROM removed_sessions WHERE id IN (
             SELECT north FROM logical_sessions
             UNION SELECT json_extract(data, '$.compacted_from') FROM logical_sessions
             UNION SELECT json_extract(boundary.value, '$.session')
                   FROM logical_sessions, json_each(stats, '$.turn_boundaries') AS boundary
         );
         WITH RECURSIVE chain(id) AS (
             SELECT coalesce(json_extract(data, '$.head'), json_extract(data, '$.session.head'))
             FROM sessions WHERE id IN (SELECT id FROM removed_sessions)
             UNION SELECT node_id FROM session_node_owners WHERE session_id IN (SELECT id FROM removed_sessions)
             UNION SELECT nodes.parent FROM nodes JOIN chain ON nodes.id = chain.id
             WHERE nodes.parent IS NOT NULL
         )
         INSERT OR IGNORE INTO removed_nodes SELECT id FROM chain;
         DELETE FROM sessions WHERE id IN (SELECT id FROM removed_sessions);
         DELETE FROM session_compactions WHERE session_id IN (SELECT id FROM removed_sessions);
         WITH RECURSIVE kept(id) AS (
             SELECT coalesce(json_extract(data, '$.head'), json_extract(data, '$.session.head')) FROM sessions
             UNION SELECT nodes.parent FROM nodes JOIN kept ON nodes.id = kept.id
             WHERE nodes.parent IS NOT NULL
         )
         DELETE FROM removed_nodes WHERE id IN (SELECT id FROM kept);",
    )?;
    collect_blobs(tx, key)?;
    for sql in [
        "DELETE FROM costs WHERE session = ?1 OR session IN (SELECT id FROM removed_sessions)",
        "DELETE FROM requests WHERE session = ?1 OR session IN (SELECT id FROM removed_sessions)",
        "DELETE FROM effects WHERE session = ?1 OR session IN (SELECT id FROM removed_sessions)",
        "DELETE FROM questions WHERE session = ?1 OR session IN (SELECT id FROM removed_sessions)",
    ] {
        tx.execute(sql, [key])?;
    }
    tx.execute_batch(
        "DROP TABLE removed_sessions; DROP TABLE removed_nodes; DROP TABLE removed_blobs;",
    )?;
    Ok(())
}

fn collect_blobs(tx: &Transaction<'_>, key: &str) -> Result<(), Fault> {
    tx.execute(
        "INSERT OR IGNORE INTO removed_blobs SELECT blob_id FROM raep_uploads WHERE session_id = ?1",
        [key],
    )?;
    tx.execute("DELETE FROM raep_uploads WHERE session_id = ?1", [key])?;
    tx.execute_batch(
        "INSERT OR IGNORE INTO removed_blobs
         SELECT media_blob_parts.blob_id
         FROM media_blob_parts JOIN removed_nodes ON removed_nodes.id = media_blob_parts.node_id
         WHERE media_blob_parts.blob_id IS NOT NULL;
         DELETE FROM nodes WHERE id IN (SELECT id FROM removed_nodes);
         DELETE FROM blobs
         WHERE id IN (SELECT id FROM removed_blobs)
           AND NOT EXISTS (SELECT 1 FROM raep_uploads WHERE blob_id = blobs.id)
           AND NOT EXISTS (SELECT 1 FROM raep_global_artifacts WHERE blob_id = blobs.id)
           AND id NOT IN (SELECT blob_id FROM media_blob_parts WHERE blob_id IS NOT NULL)
           AND id NOT IN (
               SELECT blob_id FROM pending_steer_blobs WHERE blob_id IS NOT NULL
           );",
    )?;
    Ok(())
}

pub(super) const MEDIA_VIEW: &str = "
CREATE VIEW IF NOT EXISTS media_blob_parts AS
SELECT nodes.id AS node_id,
       CASE json_extract(part.value, '$.source.source')
         WHEN 'blob' THEN json_extract(part.value, '$.source.value')
         WHEN 'provider_file' THEN json_extract(part.value, '$.source.value.blob')
       END AS blob_id
FROM nodes, json_tree(nodes.data, '$.message.parts') AS part
WHERE CASE WHEN part.type = 'object' THEN json_extract(part.value, '$.type') = 'media' END;
CREATE VIEW IF NOT EXISTS pending_steer_blobs AS
SELECT logical_sessions.key AS session_id, CASE json_extract(media.value, '$.source.source')
         WHEN 'blob' THEN json_extract(media.value, '$.source.value')
         WHEN 'provider_file' THEN json_extract(media.value, '$.source.value.blob')
         ELSE json_extract(media.value, '$.blob')
       END AS blob_id
FROM logical_sessions, json_tree(stats, '$.pending_steers') AS media
WHERE media.type = 'object';
";
