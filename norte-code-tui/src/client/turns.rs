//! Turning a session's turns: reasoning, cancelling, rewinding and pinning.

use std::sync::Arc;

use norte_code_client::AgentClient;
use norte_code_wire::SessionKey;

use super::{ClientEvent, ClientRuntime};
use crate::ui_model::ReasoningLevel;

impl ClientRuntime {
    pub(crate) fn set_reasoning(
        &self,
        session: SessionKey,
        level: ReasoningLevel,
        generation: u64,
    ) {
        let client = Arc::clone(&self.client);
        self.spawn_stream(session.clone(), generation, async move {
            client
                .set_reasoning(session.as_str(), level.to_wire())
                .await
        });
    }

    pub(crate) fn cancel_turn(&self, session: SessionKey, turn_id: String, generation: u64) {
        let client = Arc::clone(&self.client);
        let sender = self.sender.clone();
        self.runtime.spawn(async move {
            let Some(turn) = norte_code_client::TurnId::new(turn_id).ok() else {
                let _ = sender
                    .send(ClientEvent::Failure {
                        session,
                        generation,
                        message: "The running turn has no usable id.".to_owned(),
                    })
                    .await;
                return;
            };
            if let Err(error) = client.cancel_turn(session.as_str(), &turn).await {
                let _ = sender
                    .send(ClientEvent::Failure {
                        session,
                        generation,
                        message: error.to_string(),
                    })
                    .await;
            }
        });
    }

    pub(crate) fn rewind(&self, session: SessionKey, turn_id: String, generation: u64) {
        let client = Arc::clone(&self.client);
        let sender = self.sender.clone();
        self.runtime.spawn(async move {
            match client
                .rewind(session.as_str(), turn_id.as_str(), true)
                .await
            {
                Ok(removed) => {
                    let _ = sender
                        .send(ClientEvent::Rewound {
                            session,
                            generation,
                            removed,
                        })
                        .await;
                }
                Err(error) => {
                    let _ = sender
                        .send(ClientEvent::Failure {
                            session,
                            generation,
                            message: error.to_string(),
                        })
                        .await;
                }
            }
        });
    }

    pub(crate) fn pin_turns(
        &self,
        session: SessionKey,
        turn_id: String,
        pinned: bool,
        generation: u64,
    ) {
        let client = Arc::clone(&self.client);
        let sender = self.sender.clone();
        self.runtime.spawn(async move {
            let turn_ids = vec![turn_id];
            match client.pin_turns(session.as_str(), &turn_ids, pinned).await {
                Ok(pinned_turns) => {
                    let _ = sender
                        .send(ClientEvent::Pinned {
                            session,
                            generation,
                            pinned_turns,
                        })
                        .await;
                }
                Err(error) => {
                    let _ = sender
                        .send(ClientEvent::Failure {
                            session,
                            generation,
                            message: error.to_string(),
                        })
                        .await;
                }
            }
        });
    }
}
