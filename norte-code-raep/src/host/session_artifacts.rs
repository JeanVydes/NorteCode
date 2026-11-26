use std::collections::{HashMap, HashSet};

use norte_code_wire::{ArtifactOrigin, SessionArtifact, SessionKey};

use super::{HostError, NorteCodeRaepHost};

impl NorteCodeRaepHost {
    pub(crate) async fn session_artifacts(
        &self,
        session: &str,
        principal: &str,
    ) -> Result<Option<Vec<SessionArtifact>>, HostError> {
        if self.session_summary(session, principal).await.is_none() {
            return Ok(None);
        }
        let service = self.service_for_session(session)?;
        let history = service
            .session_artifacts(&SessionKey::new(session.to_owned()))
            .await
            .map_err(|error| HostError::Service(error.to_string()))?;
        let indexed = match self.upload_index.as_ref() {
            Some(index) => index.for_session(session, Some(principal)).await?,
            None => Vec::new(),
        };
        let mut preferred = HashMap::new();
        let mut inventory = Vec::with_capacity(indexed.len());
        for row in indexed {
            let item = row.summary();
            if row.referenced
                && let Some(origin) = IndexedOrigin::of(&item.origin)
            {
                preferred.insert(
                    (item.blob_id.clone(), item.mime_type.clone(), origin),
                    inventory.len(),
                );
            }
            inventory.push(item);
        }
        let mut seen_history = HashSet::new();
        for item in history {
            let row = IndexedOrigin::of(&item.origin).and_then(|origin| {
                preferred.get(&(item.blob_id.clone(), item.mime_type.clone(), origin))
            });
            if let Some(row) = row {
                if inventory[*row].name.is_none() {
                    inventory[*row].name = item.name;
                }
            } else if seen_history.insert((
                item.blob_id.clone(),
                item.mime_type.clone(),
                item.origin.clone(),
            )) {
                inventory.push(item);
            }
        }
        inventory.sort_by(|left, right| {
            right
                .created_at
                .cmp(&left.created_at)
                .then_with(|| left.id.as_str().cmp(right.id.as_str()))
        });
        Ok(Some(inventory))
    }

    pub(crate) async fn session_artifact_bytes(
        &self,
        session: &str,
        id: &str,
        principal: &str,
    ) -> Result<Option<(Vec<u8>, String)>, HostError> {
        if self.session_summary(session, principal).await.is_none() {
            return Ok(None);
        }
        if let Some(index) = &self.upload_index
            && let Some(row) = index.get(id).await?
            && row.visible(session, Some(principal))
        {
            let store = self
                .artifact_store
                .as_ref()
                .ok_or_else(|| HostError::Artifact("artifact storage is disabled".to_owned()))?;
            return Ok(Some((store.get(&row.blob_id).await?, row.mime_type)));
        }
        self.service_for_session(session)?
            .session_artifact_bytes(&SessionKey::new(session.to_owned()), id)
            .await
            .map_err(|error| HostError::Service(error.to_string()))
    }
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
enum IndexedOrigin {
    Upload,
    Output,
}

impl IndexedOrigin {
    fn of(origin: &ArtifactOrigin) -> Option<Self> {
        match origin {
            ArtifactOrigin::UserUpload => Some(Self::Upload),
            ArtifactOrigin::AssistantOutput { .. } => Some(Self::Output),
            ArtifactOrigin::Plugin { .. }
            | ArtifactOrigin::Tool { .. }
            | ArtifactOrigin::Message => None,
        }
    }
}
