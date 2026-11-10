use crate::runtime::Conversation;
const CONVERSATION_SCHEMA_VERSION: u16 = 1;
/// The session surface: creating a session, reading it, seeding it with a
/// history the host already holds, and moving it in and out of a portable
/// envelope.
use crate::runtime::Commit;

use crate::runtime::Node;
use crate::runtime::Phase;
use crate::runtime::Session;
use crate::runtime::event::EngineEvent;
use norte::domain::message::Message;
use norte::error::{Error, Result};
use norte::id::SessionId;
use norte::model::TURN_OPTIONS_KEY;

use super::Harness;

impl Harness {
    /// Creates an empty session (extension metadata records the registered
    /// observers and layers so a run stays reproducible).
    ///
    /// # Errors
    ///
    /// Propagates the store error when the initial commit fails.
    pub async fn create(&self) -> Result<Session> {
        let mut session = Session::new();
        session.metadata.insert(
            "observers".to_owned(),
            serde_json::json!(self.observers.names()),
        );
        session.metadata.insert(
            "layers".to_owned(),
            serde_json::json!(
                self.layers
                    .layers()
                    .iter()
                    .map(|layer| layer.name().to_owned())
                    .collect::<Vec<_>>()
            ),
        );
        session.metadata.insert(
            "transforms".to_owned(),
            serde_json::json!(self.observers.transform_names()),
        );
        session.metadata.insert(
            "interceptors".to_owned(),
            serde_json::json!(self.interceptors.names()),
        );
        let commit = Commit {
            session_id: session.id,
            expected_version: None,
            nodes: Vec::new(),
            head: None,
            phase: Phase::Idle,
            turn: 0,
            metadata: Some(session.metadata.clone()),
        };
        let session = self.store.commit(commit).await?;
        self.observers.emit(&EngineEvent::SessionCreated {
            session: session.id,
            phase: session.phase,
        });
        Ok(session)
    }

    /// Loads the current session state.
    ///
    /// # Errors
    ///
    /// Returns the store's error, including [`Error::SessionNotFound`].
    pub async fn session(&self, session: SessionId) -> Result<Session> {
        self.store.load(session).await
    }

    /// Loads the ancestor conversation for the session head.
    ///
    /// # Errors
    ///
    /// Propagates the store error while walking the graph.
    pub async fn history(&self, session: SessionId) -> Result<Vec<Message>> {
        let session = self.store.load(session).await?;
        self.store.history(session.head).await
    }

    /// Appends `messages` to the session as durable history, in order, without
    /// generating (FR-ENG-6).
    ///
    /// This is the array-to-graph bridge: a host that already holds a
    /// conversation as an ordered `Vec<Message>`: a transcript it loaded,
    /// imported from another system or built in memory: persists it verbatim
    /// onto the current head. Nothing is sent to the model, so the next
    /// [`send`](Harness::send) sees the seeded prefix unchanged and the provider
    /// prompt cache keeps hitting (FR-RECON-2).
    ///
    /// Appending requires an `Idle` session: a running turn owns the head, and
    /// moving it underneath the turn would race its commit. An empty slice is a
    /// no-op that returns the session unchanged.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidInput`] when the session is not `Idle`, and
    /// propagates the store error, including [`Error::StoreConflict`] when the
    /// session advanced under the caller (FR-ENG-2).
    pub async fn append_history(
        &self,
        session: SessionId,
        messages: Vec<Message>,
    ) -> Result<Session> {
        let current = self.store.load(session).await?;
        if messages.is_empty() {
            return Ok(current);
        }
        if current.phase != Phase::Idle {
            return Err(Error::InvalidInput(format!(
                "append_history requires an Idle session, found {:?}",
                current.phase
            )));
        }

        let mut parent = current.head;
        let mut nodes = Vec::with_capacity(messages.len());
        for message in messages {
            let node = Node::message(message, parent);
            parent = Some(node.id);
            nodes.push(node);
        }
        let appended = nodes.len();
        let committed = self
            .store
            .commit(Commit {
                session_id: current.id,
                expected_version: Some(current.version),
                nodes,
                head: parent,
                phase: current.phase,
                turn: current.turn,
                metadata: None,
            })
            .await?;
        self.observers.emit(&EngineEvent::HistoryAppended {
            session: committed.id,
            messages: appended,
        });
        Ok(committed)
    }

    #[allow(missing_docs, clippy::missing_errors_doc)]
    pub async fn export(&self, session: SessionId) -> Result<Conversation> {
        let state = self.store.load(session).await?;
        let messages = self.store.history(state.head).await?;
        let mut metadata = state.metadata;
        let options = metadata
            .remove(TURN_OPTIONS_KEY)
            .and_then(|value| serde_json::from_value(value).ok());
        Ok(Conversation {
            schema_version: CONVERSATION_SCHEMA_VERSION,
            session: state.id,
            phase: state.phase,
            turn: state.turn,
            metadata,
            options,
            messages,
        })
    }

    /// Commits a [`Conversation`] as a brand-new session under the same id, so
    /// the next generation reproduces the exact same wire bytes (FR-EXP-2).
    ///
    /// The messages are committed verbatim as history (no generation), chained
    /// onto the new session's head. The store rejects the import when the id
    /// already exists, so a restore never clobbers a live session. Importing
    /// under a different id than the source is an explicit cache-invalidating
    /// fork; the caller decides that by editing [`Conversation::session`]
    /// first.
    ///
    /// Byte-identity to what was previously sent holds when the importing
    /// engine is configured with the same model, capabilities, tools and
    /// generation parameters: reconstruction depends only on those plus the
    /// history, and the history is preserved losslessly.
    ///
    /// # Errors
    ///
    /// Returns [`Error::SchemaVersion`] for an envelope this build does not
    /// understand, and propagates the store error, including
    /// [`Error::StoreConflict`] when the id already exists.
    #[allow(missing_docs, clippy::missing_errors_doc)]
    pub async fn import(&self, conversation: Conversation) -> Result<Session> {
        if conversation.schema_version != CONVERSATION_SCHEMA_VERSION {
            return Err(Error::SchemaVersion {
                kind: "conversation".to_owned(),
                found: conversation.schema_version,
            });
        }

        let mut parent = None;
        let mut nodes = Vec::with_capacity(conversation.messages.len());
        for message in conversation.messages {
            let node = Node::message(message, parent);
            parent = Some(node.id);
            nodes.push(node);
        }

        let mut metadata = conversation.metadata;
        if let Some(options) = &conversation.options
            && let Ok(value) = serde_json::to_value(options)
        {
            metadata.insert(TURN_OPTIONS_KEY.to_owned(), value);
        }

        let committed = self
            .store
            .commit(Commit {
                session_id: conversation.session,
                expected_version: None,
                nodes,
                head: parent,
                phase: conversation.phase,
                turn: conversation.turn,
                metadata: Some(metadata),
            })
            .await?;
        Ok(committed)
    }
}
