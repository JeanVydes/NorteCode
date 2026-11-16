//! Pinning the turns that stay verbatim through compaction.
//!
//! The list is persisted with the session's stats so a compaction sees the
//! same pins the client set, and so a restart keeps them. The RAEP host
//! forwards `CONTEXT_PIN` here so the pins it echoes and the pins the
//! compaction seeds keep agree.

use norte_code_wire::SessionKey;

use super::{Service, ServiceError};

/// How many turns may be pinned per session.
pub const MAX_PINNED_TURNS: usize = 20;

impl Service {
    /// Pins or unpins `turn_ids` for `key` and persists the result.
    ///
    /// The request is all-or-nothing: pinning turns that would grow the
    /// list past [`MAX_PINNED_TURNS`] pins none of them.
    ///
    /// Returns the pinned ids after the change, sorted the way a caller can
    /// show or forward them.
    ///
    /// # Errors
    ///
    /// Returns an error when the session is unknown, the pin list overflows,
    /// or the record cannot be persisted.
    pub async fn pin_turns(
        &self,
        key: &SessionKey,
        turn_ids: &[String],
        pinned: bool,
    ) -> Result<Vec<String>, ServiceError> {
        {
            let mut sessions = self.lock_sessions();
            let record = sessions
                .get_mut(key)
                .ok_or_else(|| ServiceError::UnknownSession(key.to_string()))?;
            if pinned {
                let fresh = turn_ids
                    .iter()
                    .filter(|id| !record.pinned_turn_ids.contains(id.as_str()))
                    .count();
                if record.pinned_turn_ids.len() + fresh > MAX_PINNED_TURNS {
                    // All or nothing: pinning the new turns would grow the
                    // set past the cap, so none of them are pinned.
                    return Err(ServiceError::History(
                        "PAYLOAD_TOO_LARGE: too many pinned turns".to_owned(),
                    ));
                }
                for id in turn_ids {
                    record.pinned_turn_ids.insert(id.clone());
                }
            } else {
                for id in turn_ids {
                    record.pinned_turn_ids.remove(id);
                }
            }
        }
        self.persist(key).await?;
        Ok(self.pinned_turns(key))
    }

    /// The pinned turns for `key`, sorted.
    #[must_use]
    pub fn pinned_turns(&self, key: &SessionKey) -> Vec<String> {
        let sessions = self.lock_sessions();
        sessions.get(key).map_or_else(Vec::new, |record| {
            let mut pins: Vec<String> = record.pinned_turn_ids.iter().cloned().collect();
            pins.sort();
            pins
        })
    }
}
