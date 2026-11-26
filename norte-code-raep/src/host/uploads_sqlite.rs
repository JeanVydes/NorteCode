use std::path::PathBuf;

use rusqlite::{Connection, OptionalExtension, Row, params};

use super::{
    ArtifactRef, ArtifactStore, HashMap, HostError, UploadIndex, UploadRecord, now_millis,
};
use async_trait::async_trait;

/// SQLite-backed index sharing the production session database.
pub struct SqliteUploadIndex {
    path: PathBuf,
}

impl SqliteUploadIndex {
    /// Opens an index in a database initialized by the session store.
    #[must_use]
    pub fn open(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Loads persisted session owners before the host accepts requests.
    ///
    /// # Errors
    ///
    /// Returns a storage error if the ownership index cannot be read.
    pub async fn session_owners(&self) -> Result<HashMap<String, String>, HostError> {
        self.run(|connection| {
            let mut statement =
                connection.prepare("SELECT session_id, owner FROM raep_session_owners")?;
            statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect()
        })
        .await
    }

    async fn run<T: Send + 'static>(
        &self,
        work: impl FnOnce(&mut Connection) -> rusqlite::Result<T> + Send + 'static,
    ) -> Result<T, HostError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let mut connection = Connection::open(path)?;
            connection.busy_timeout(std::time::Duration::from_secs(5))?;
            work(&mut connection)
        })
        .await
        .map_err(|error| HostError::Artifact(error.to_string()))?
        .map_err(|error| HostError::Artifact(error.to_string()))
    }
}

#[async_trait]
impl UploadIndex for SqliteUploadIndex {
    async fn insert(&self, record: UploadRecord) -> Result<(), HostError> {
        self.run(move |connection| insert(connection, &record))
            .await
    }

    async fn store(
        &self,
        _: &dyn ArtifactStore,
        record: UploadRecord,
        bytes: &[u8],
    ) -> Result<(), HostError> {
        let bytes = bytes.to_vec();
        self.run(move |connection| {
            let tx = connection.transaction()?;
            tx.execute(
                "INSERT OR IGNORE INTO blobs (id, bytes) VALUES (?1, ?2)",
                params![record.blob_id, bytes],
            )?;
            let exists = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM logical_sessions WHERE key = ?1)",
                [&record.session_id],
                |row| row.get::<_, bool>(0),
            )?;
            if !exists {
                return Err(rusqlite::Error::QueryReturnedNoRows);
            }
            insert(&tx, &record)?;
            tx.commit()
        })
        .await
    }

    async fn claim_session(&self, session: &str, owner: &str) -> Result<(), HostError> {
        let session = session.to_owned();
        let owner = owner.to_owned();
        self.run(move |connection| {
            connection
                .execute(
                    "INSERT OR IGNORE INTO raep_session_owners (session_id, owner)
                 SELECT key, ?2 FROM logical_sessions WHERE key = ?1",
                    params![session, owner],
                )
                .map(|_| ())
        })
        .await
    }

    async fn store_global(
        &self,
        mime: &str,
        bytes: &[u8],
        owner: &str,
    ) -> Result<Option<ArtifactRef>, HostError> {
        let id = norte::BlobId::of(bytes).to_string();
        let reference = ArtifactRef {
            id: id.clone(),
            mime_type: mime.to_owned(),
            bytes: bytes.len() as u64,
        };
        let bytes = bytes.to_vec();
        let owner = owner.to_owned();
        self.run(move |connection| {
            let tx = connection.transaction()?;
            tx.execute(
                "INSERT OR IGNORE INTO blobs (id, bytes) VALUES (?1, ?2)",
                params![id, bytes],
            )?;
            tx.execute(
                "INSERT OR IGNORE INTO raep_global_artifacts (blob_id, owner) VALUES (?1, ?2)",
                params![id, owner],
            )?;
            tx.commit()
        })
        .await?;
        Ok(Some(reference))
    }

    async fn owns_global(&self, id: &str, owner: &str) -> Result<bool, HostError> {
        let id = id.to_owned();
        let owner = owner.to_owned();
        self.run(move |connection| {
            connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM raep_global_artifacts WHERE blob_id = ?1 AND owner = ?2)",
            params![id, owner], |row| row.get(0),
        )
        })
        .await
    }

    async fn for_session(
        &self,
        session: &str,
        owner: Option<&str>,
    ) -> Result<Vec<UploadRecord>, HostError> {
        let session = session.to_owned();
        let owner = owner.map(str::to_owned);
        self.run(move |connection| {
            let mut statement = connection.prepare(
                "SELECT upload_id, session_id, owner, blob_id, mime_type, bytes, sha256, expires_at, referenced, details
                 FROM raep_uploads WHERE session_id = ?1 AND owner IS ?2 AND (referenced = 1 OR expires_at > ?3)"
            )?;
            statement.query_map(params![session, owner, now_millis()], decode_record)?.collect()
        }).await
    }

    async fn delete_session(&self, _: &str) -> Result<(), HostError> {
        // The SQLite session store removes index rows and collects their blobs in its transaction.
        Ok(())
    }

    async fn get(&self, id: &str) -> Result<Option<UploadRecord>, HostError> {
        let id = id.to_owned();
        self.run(move |connection| {
            connection.query_row(
                "SELECT upload_id, session_id, owner, blob_id, mime_type, bytes, sha256, expires_at, referenced, details
                 FROM raep_uploads WHERE upload_id = ?1", [id], decode_record,
            ).optional()
        }).await
    }

    async fn mark_referenced(
        &self,
        session_id: &str,
        owner: Option<&str>,
        ids: &[String],
    ) -> Result<(), HostError> {
        let session_id = session_id.to_owned();
        let owner = owner.map(str::to_owned);
        let ids = ids.to_vec();
        self.run(move |connection| {
            let transaction = connection.transaction()?;
            for id in ids {
                let updated = transaction.execute(
                    "UPDATE raep_uploads SET referenced = 1
                     WHERE upload_id = ?1 AND session_id = ?2 AND owner IS ?3
                       AND (referenced = 1 OR expires_at > ?4)",
                    params![id, session_id, owner, now_millis()],
                )?;
                if updated != 1 {
                    return Err(rusqlite::Error::QueryReturnedNoRows);
                }
            }
            transaction.commit()
        })
        .await
    }

    async fn clear(&self) -> Result<(), HostError> {
        self.run(|connection| {
            connection
                .execute("DELETE FROM raep_uploads", [])
                .map(|_| ())
        })
        .await
    }
}

fn insert(connection: &Connection, record: &UploadRecord) -> rusqlite::Result<()> {
    let details = record
        .details
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    connection.execute(
        "INSERT INTO raep_uploads
         (upload_id, session_id, owner, blob_id, mime_type, bytes, sha256, expires_at, referenced, details)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![record.id, record.session_id, record.owner, record.blob_id, record.mime_type,
            record.bytes, record.sha256, record.expires_at, record.referenced, details],
    )?;
    Ok(())
}

fn decode_record(row: &Row<'_>) -> rusqlite::Result<UploadRecord> {
    let details = row
        .get::<_, Option<String>>(9)?
        .map(|value| serde_json::from_str(&value))
        .transpose()
        .map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                9,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?;
    Ok(UploadRecord {
        id: row.get(0)?,
        session_id: row.get(1)?,
        owner: row.get(2)?,
        blob_id: row.get(3)?,
        mime_type: row.get(4)?,
        bytes: row.get(5)?,
        sha256: row.get(6)?,
        expires_at: row.get(7)?,
        referenced: row.get(8)?,
        details,
    })
}
