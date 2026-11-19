//! Importing a session from a bundle.

use std::{path::PathBuf, sync::Arc};

use norte_code_wire::SessionKey;

use super::{ClientEvent, ClientRuntime};

impl ClientRuntime {
    /// Reads a bundle and asks the host to turn it into a session.
    pub(crate) fn import_session(&self, session: SessionKey, path: PathBuf, generation: u64) {
        let api = Arc::clone(&self.api);
        let sender = self.sender.clone();
        self.runtime.spawn(async move {
            let bytes = match tokio::fs::read(&path).await {
                Ok(bytes) => bytes,
                Err(error) => {
                    let _ = sender
                        .send(ClientEvent::Failure {
                            session,
                            generation,
                            message: format!("Could not read {}: {error}", path.display()),
                        })
                        .await;
                    return;
                }
            };
            let preview = match api.import_bundle(bytes).await {
                Ok(preview) => preview,
                Err(error) => {
                    let _ = sender
                        .send(ClientEvent::Failure {
                            session,
                            generation,
                            message: format!("The bundle was refused: {error}"),
                        })
                        .await;
                    return;
                }
            };
            let import_id = preview.import_id.clone();
            let imported = match api.commit_import(&import_id, None, true).await {
                Ok(imported) => imported,
                Err(error) => {
                    let _ = sender
                        .send(ClientEvent::Failure {
                            session,
                            generation,
                            message: format!("The import was refused: {error}"),
                        })
                        .await;
                    return;
                }
            };
            let _ = sender
                .send(ClientEvent::Imported {
                    session,
                    generation,
                    session_id: imported.session_id,
                })
                .await;
        });
    }
}
