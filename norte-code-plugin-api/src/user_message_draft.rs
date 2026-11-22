use norte::domain::part::Part;
use norte::{Error, Result};

/// A user-role message a plugin asks the host to add to the conversation.
///
/// The host decides whether to apply it. The draft only describes what the
/// plugin would like the model to see.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "UserMessageWire", into = "UserMessageWire")]
pub struct UserMessageDraft {
    parts: Vec<Part>,
}

impl UserMessageDraft {
    /// Builds a draft from message parts.
    ///
    /// Only non-empty text and media are allowed.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidInput`] when the parts are empty or carry a kind
    /// a plugin may not ask the host to inject.
    pub fn new(parts: Vec<Part>) -> Result<Self> {
        if parts.is_empty() {
            return Err(Error::InvalidInput(
                "a user message effect needs at least one part".to_owned(),
            ));
        }
        for part in &parts {
            match part {
                Part::Media(_) => {}
                Part::Text(text) if !text.text.trim().is_empty() => {}
                _ => {
                    return Err(Error::InvalidInput(
                        "a user message effect carries only non-empty text or media".to_owned(),
                    ));
                }
            }
        }
        Ok(Self { parts })
    }

    /// Builds a draft carrying one text part.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidInput`] when the text is empty.
    pub fn text(text: impl Into<String>) -> Result<Self> {
        Self::new(vec![Part::text(text)])
    }

    /// The validated parts.
    #[must_use]
    pub fn parts(&self) -> &[Part] {
        &self.parts
    }

    /// Takes the parts for the host to adapt and commit.
    #[must_use]
    pub fn into_parts(self) -> Vec<Part> {
        self.parts
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct UserMessageWire {
    parts: Vec<Part>,
}

impl TryFrom<UserMessageWire> for UserMessageDraft {
    type Error = Error;

    fn try_from(wire: UserMessageWire) -> Result<Self> {
        Self::new(wire.parts)
    }
}

impl From<UserMessageDraft> for UserMessageWire {
    fn from(draft: UserMessageDraft) -> Self {
        Self { parts: draft.parts }
    }
}
