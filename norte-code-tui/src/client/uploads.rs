//! Uploading a file to attach it to the next message.

use std::{path::PathBuf, sync::Arc};

use norte_code_wire::{InputPartKind, SessionKey};

use super::{ClientEvent, ClientRuntime};
use crate::chips::{Chip, ChipContent};

/// The largest file the client reads.
const MAX_BYTES: u64 = 16 * 1024 * 1024;

impl ClientRuntime {
    /// Reads a file and hands it to the host.
    ///
    /// The host stores the bytes and answers with an upload id, which is
    /// what travels with the message.
    pub(crate) fn upload_file(
        &self,
        session: SessionKey,
        path: PathBuf,
        mime: String,
        kind: InputPartKind,
        generation: u64,
    ) {
        let api = Arc::clone(&self.api);
        let client = Arc::clone(&self.client);
        let sender = self.sender.clone();
        self.runtime.spawn(async move {
            let name = path.file_name().map_or_else(
                || path.display().to_string(),
                |name| name.to_string_lossy().to_string(),
            );
            let bytes = match tokio::fs::read(&path).await {
                Ok(bytes) => bytes,
                Err(error) => {
                    let _ = sender
                        .send(ClientEvent::Failure {
                            session,
                            generation,
                            message: format!("Could not read {name}: {error}"),
                        })
                        .await;
                    return;
                }
            };
            if bytes.len() as u64 > MAX_BYTES {
                let _ = sender
                    .send(ClientEvent::Failure {
                        session,
                        generation,
                        message: format!(
                            "{name} is larger than the {MAX_BYTES}-byte limit of the client."
                        ),
                    })
                    .await;
                return;
            }
            let receipt = match api.upload(session.as_str(), &mime, bytes.clone()).await {
                Err(error) if error.is_session_not_attached() => {
                    if client.recover_attachment(session.as_str()).await.is_err() {
                        let _ = sender
                            .send(ClientEvent::Failure {
                                session,
                                generation,
                                message: format!("Uploading {name} failed: {error}"),
                            })
                            .await;
                        return;
                    }
                    match api.upload(session.as_str(), &mime, bytes).await {
                        Ok(receipt) => receipt,
                        Err(error) => {
                            let _ = sender
                                .send(ClientEvent::Failure {
                                    session,
                                    generation,
                                    message: format!("Uploading {name} failed: {error}"),
                                })
                                .await;
                            return;
                        }
                    }
                }
                Ok(receipt) => receipt,
                Err(error) => {
                    let _ = sender
                        .send(ClientEvent::Failure {
                            session,
                            generation,
                            message: format!("Uploading {name} failed: {error}"),
                        })
                        .await;
                    return;
                }
            };
            let part = norte_code_client::upload_part(kind, &receipt.upload_id, &receipt.mime_type);
            let detail = format!("{} bytes", receipt.bytes);
            let chip = Chip {
                content: ChipContent::Upload(part),
                label: name,
                detail,
                kind: Some(kind),
            };
            let _ = sender
                .send(ClientEvent::Uploaded {
                    session,
                    generation,
                    chip,
                })
                .await;
        });
    }
}
