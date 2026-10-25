"""Let the client say a session already existed, instead of it being guessed.

The replay hangs on `input.is_none()`, which is true for a resume and false for a
message. That works for `resume` and for a brand-new session, and it fails for
the third case a person actually hits: `run --session KEY` with a prompt, which
sends a message to a session that already has a history. The client knew; the
service was guessing from the shape of the request.
"""

import pathlib

path = pathlib.Path("norte-code-protocol/src/lib.rs")
text = path.read_text()

OLD = """    /// Start a turn. With no session, one is created first.
    SendMessage {
        /// The session, or `None` to create one.
        session: Option<SessionKey>,
        /// The user message.
        text: String,
        /// A budget for this message only.
        budget: Option<Budget>,
        /// A reasoning override for this message only.
        reasoning: Option<Reasoning>,
    },"""

NEW = """    /// Start a turn. With no session, one is created first.
    SendMessage {
        /// The session, or `None` to create one.
        session: Option<SessionKey>,
        /// The user message.
        text: String,
        /// A budget for this message only.
        budget: Option<Budget>,
        /// A reasoning override for this message only.
        reasoning: Option<Reasoning>,
        /// Whether the client is attaching to a session that already has a
        /// history, and wants it replayed before the answer.
        ///
        /// A client that starts blank cannot tell what the session holds, so it
        /// asks for the transcript. The service used to infer this from the
        /// request having no message, which is true for a resume and false for
        /// `run --session KEY`: a message sent to an existing session got no
        /// transcript at all.
        #[serde(default)]
        replay: bool,
    },"""

assert text.count(OLD) == 1, text.count(OLD)
path.write_text(text.replace(OLD, NEW))
print("ok")
