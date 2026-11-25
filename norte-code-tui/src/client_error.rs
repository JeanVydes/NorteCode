//! Typed failures the connection reports to the interface.
//!
//! A reader sees the message; the variant lets the app classify it without
//! parsing human text.

use std::error::Error;
use std::fmt;
use std::path::Path;

#[cfg(test)]
#[path = "client_error_tests.rs"]
mod tests;

/// A failure from one specific host operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ClientError {
    /// Cancelling the running turn.
    CancelTurn(String),
    /// Renaming a session.
    RenameSession(String),
    /// Deleting a session.
    DeleteSession(String),
    /// Clearing persisted service data.
    ClearData(String),
    /// Installing, updating or toggling a plugin.
    Plugin(String),
    /// Creating a session.
    CreateSession(String),
    /// Starting the async runtime.
    Runtime(String),
    /// Building the stream client.
    RemoteClient(String),
    /// Building the REST client.
    RestClient(String),
    /// Listing sessions during attach.
    ListSessions(String),
    /// Sending a prompt.
    SendPrompt(String),
    /// Reading an attachment from disk.
    ReadAttachment(String),
    /// An attachment exceeded the host's size limit.
    AttachmentTooLarge(String),
    /// Uploading an attachment.
    Upload(String),
    /// Answering a host question.
    AnswerQuestion(String),
    /// Compacting a session.
    Compact(String),
    /// Forking a session.
    Fork(String),
    /// Rewinding a session.
    Rewind(String),
    /// The event stream closed before it became healthy.
    UnstableStream,
    /// Opening the event stream failed.
    LostStream(String),
    /// Fetching a session's history.
    History(String),
    /// Storing a provider key on the host.
    SaveSecret(String),
    /// Removing a provider key from the host.
    DeleteSecret(String),
}

impl ClientError {
    /// The host refused to cancel the running turn.
    pub(crate) fn cancel(error: impl fmt::Display) -> Self {
        Self::CancelTurn(format!("cancel refused: {error}"))
    }

    /// The running turn's id could not be parsed.
    pub(crate) fn invalid_turn(error: impl fmt::Display) -> Self {
        Self::CancelTurn(format!(
            "cancel refused: the running turn has an unusable id: {error}"
        ))
    }

    /// The host refused a rename.
    pub(crate) fn rename(error: impl fmt::Display) -> Self {
        Self::RenameSession(format!("rename session failed · {error}"))
    }

    /// The host refused a delete.
    pub(crate) fn delete(error: impl fmt::Display) -> Self {
        Self::DeleteSession(format!("delete session failed · {error}"))
    }

    /// The host refused to clear service data.
    pub(crate) fn clear_data(error: impl fmt::Display) -> Self {
        Self::ClearData(format!("clear data failed · {error}"))
    }

    /// A plugin operation failed.
    pub(crate) fn plugin(error: impl fmt::Display) -> Self {
        Self::Plugin(format!("plugin operation failed · {error}"))
    }

    /// Creating a session failed.
    pub(crate) fn create_session(error: impl fmt::Display) -> Self {
        Self::CreateSession(format!("could not create a session: {error}"))
    }

    /// The async runtime could not start.
    pub(crate) fn runtime(error: impl fmt::Display) -> Self {
        Self::Runtime(format!("could not start the runtime: {error}"))
    }

    /// The stream client could not be built.
    pub(crate) fn remote_client(error: impl fmt::Display) -> Self {
        Self::RemoteClient(format!("could not build the raep client: {error}"))
    }

    /// The REST client could not be built.
    pub(crate) fn rest_client(error: impl fmt::Display) -> Self {
        Self::RestClient(format!("could not build the raep api: {error}"))
    }

    /// The session list could not be fetched during attach.
    pub(crate) fn list_sessions(error: impl fmt::Display) -> Self {
        Self::ListSessions(format!("could not list the sessions: {error}"))
    }

    /// The host refused a prompt.
    pub(crate) fn send_prompt(error: impl fmt::Display) -> Self {
        Self::SendPrompt(format!("message refused: {error}"))
    }

    /// An attachment could not be read from disk.
    pub(crate) fn read_attachment(path: &Path, error: impl fmt::Display) -> Self {
        Self::ReadAttachment(format!("could not read {}: {error}", path.display()))
    }

    /// An attachment was too large for the host.
    pub(crate) fn attachment_too_large(bytes: usize, limit: u64) -> Self {
        Self::AttachmentTooLarge(format!("attachment has {bytes} bytes, limit is {limit}"))
    }

    /// The host refused an upload.
    pub(crate) fn upload(error: impl fmt::Display) -> Self {
        Self::Upload(error.to_string())
    }

    /// The host refused an answer.
    pub(crate) fn answer(error: impl fmt::Display) -> Self {
        Self::AnswerQuestion(format!("answer refused: {error}"))
    }

    /// The host refused a compaction.
    pub(crate) fn compact(error: impl fmt::Display) -> Self {
        Self::Compact(format!("compact refused: {error}"))
    }

    /// The host refused a fork.
    pub(crate) fn fork(error: impl fmt::Display) -> Self {
        Self::Fork(format!("fork failed: {error}"))
    }

    /// The host refused a rewind.
    pub(crate) fn rewind(error: impl fmt::Display) -> Self {
        Self::Rewind(format!("rewind failed: {error}"))
    }

    /// The stream kept closing before it was healthy.
    pub(crate) fn unstable_stream() -> Self {
        Self::UnstableStream
    }

    /// Opening the stream failed repeatedly.
    pub(crate) fn lost_stream(error: impl fmt::Display) -> Self {
        Self::LostStream(format!(
            "lost the stream ({error}); retrying in the background, check the raep server"
        ))
    }

    /// The session history could not be fetched.
    pub(crate) fn history(error: impl fmt::Display) -> Self {
        Self::History(error.to_string())
    }

    /// The host refused to store a provider key.
    pub(crate) fn save_secret(error: impl fmt::Display) -> Self {
        Self::SaveSecret(format!("could not save the key: {error}"))
    }

    /// The host refused to remove a provider key.
    pub(crate) fn delete_secret(error: impl fmt::Display) -> Self {
        Self::DeleteSecret(format!("could not delete the key: {error}"))
    }
}

impl fmt::Display for ClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CancelTurn(message)
            | Self::RenameSession(message)
            | Self::DeleteSession(message)
            | Self::ClearData(message)
            | Self::Plugin(message)
            | Self::CreateSession(message)
            | Self::Runtime(message)
            | Self::RemoteClient(message)
            | Self::RestClient(message)
            | Self::ListSessions(message)
            | Self::SendPrompt(message)
            | Self::ReadAttachment(message)
            | Self::AttachmentTooLarge(message)
            | Self::Upload(message)
            | Self::AnswerQuestion(message)
            | Self::Compact(message)
            | Self::Fork(message)
            | Self::Rewind(message)
            | Self::LostStream(message)
            | Self::History(message)
            | Self::SaveSecret(message)
            | Self::DeleteSecret(message) => formatter.write_str(message),
            Self::UnstableStream => formatter.write_str(
                "the stream keeps closing; retrying in the background, check the raep server",
            ),
        }
    }
}

impl Error for ClientError {}
