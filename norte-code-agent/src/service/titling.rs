//! Naming a session from its first message.
//!
//! A session is named by its key until it has a title, and a key is not a name:
//! `55fd07f3` says nothing about what the session was for, so a list of ten of
//! them is ten rows a person has to open to tell apart. The title is written
//! once, from the first message they sent — that is what the session is about,
//! and it is the only thing available before there is any answer.
//!
//! The request is its own session, thrown away after: a scratch Norte session
//! holding one instruction, built to answer one question. Nothing of it reaches
//! the session being named, so a title costs a request and never a message in
//! the conversation, and the model that answers can be a cheaper one than the
//! session's own (`titling.model`).
//!
//! It is deliberately the cheapest request the service can make: no tools, no
//! reasoning, a capped output, and the reply taken as a line of text rather than
//! as anything a host acts on. A title is a label, so what comes back is
//! sanitised — control characters, quotes and newlines are the model's to emit
//! and nobody's to trust — and a reply that is empty or absurd falls back to no
//! title at all, which is what the session had before.
//!
//! The scratch session stays in the store, as a compaction's summary session
//! does: Norte's store has no delete, so the choice is a small orphan
//! conversation or no title at all. It is one scratch session per titled
//! session, and it is the trade the feature is built on.

use norte::{Message, ReasoningRequest, Role};

use super::{ModelSpec, Service, ServiceError};

/// What the model is asked, with the person's own words after it.
///
/// It asks for the words the user would recognise it by, and it says what not to
/// do, because a model given a free hand writes a sentence: *"A summary of the
/// user's request to fix the footer"*. The two examples anchor the shape without
/// asking for a format, and the longest a title may be is stated in characters
/// rather than tokens, which is the measure a person reading a list cares about.
pub(super) const TITLE_PROMPT: &str = "\
Write a title for a conversation that starts with the message below. Answer with \
the title alone: no quotes, no punctuation at the end, no explanation, no \
sentence about what the message asks for. At most 60 characters. Name the work, \
not the request: `fix the footer overflow` rather than `the user wants the footer \
fixed`. Use the same language the message is in.";

/// The longest title accepted, in characters.
///
/// The prompt asks for sixty; this is what the host enforces. A model that
/// overruns is trimmed rather than refused — the words up to the limit are still
/// the best name available — and the limit exists because the title goes in a
/// footer and a startup row, both of which are one line.
const MAX_TITLE_CHARS: usize = 60;

/// A title from the model, or `None` when it produced nothing usable.
///
/// The text is the model's, so it is a label and nothing else: escapes are
/// dropped rather than shown, because a title reaches a terminal, and the
/// whitespace is folded because a list row is one line. A reply that is only
/// punctuation, or one that is longer than a title has any business being after
/// folding, is refused — the session keeps its key, which is honest, rather than
/// taking a name nobody can use.
pub(super) fn sanitise_title(raw: &str) -> Option<String> {
    // A model asked for one line sometimes answers with one line and a label:
    // `Title: fix the footer`. Stripping the lead-in keeps the words that were
    // asked for, and it is only dropped when what follows is non-empty.
    let text = raw.trim();
    let text = strip_label(text);
    // A newline inside a reply means more than one line came back; the first is
    // the title and the rest is commentary nobody asked for.
    let first = text.lines().next().unwrap_or("").trim();
    // A tab or any other whitespace control is separation, not noise: folding it
    // into a space keeps `a\tb` two words instead of one the model never wrote.
    let folded: String = first
        .chars()
        .filter_map(|character| match character {
            ' ' | '\t' => Some(' '),
            other if keep_in_name(other) => Some(other),
            _ => None,
        })
        .collect();
    let folded = folded.split_whitespace().collect::<Vec<_>>().join(" ");
    // Quotes are how a model wraps a title it was told not to wrap; they are
    // dropped from both ends rather than from the middle of a name that has one.
    let trimmed = folded.trim_matches(|c: char| {
        c == '"' || c == '\'' || c == '«' || c == '»' || c == '“' || c == '”'
    });
    let trimmed = trimmed.trim_end_matches(['.', ':', ';', ','].as_slice());
    let trimmed = trimmed.trim();
    if trimmed.is_empty() {
        return None;
    }
    // A reply with no letters is not a name, whatever it is.
    if !trimmed.chars().any(char::is_alphabetic) {
        return None;
    }
    Some(truncate_chars(trimmed, MAX_TITLE_CHARS))
}

/// Drops what a title must not carry, whatever the model wrote.
///
/// A title is shown in a footer and a startup row, one line each, and the text
/// is the model's. Control characters are dropped rather than escaped: an escape
/// would be visible in a name a person reads at a glance, and the character was
/// never part of the name anyway. The bidi controls are dropped for a different
/// reason — they are not control characters by Rust's measure, and one of them
/// reverses the reading order of everything after it, which in a list of
/// sessions means two rows that read as a different session than they open.
fn keep_in_name(character: char) -> bool {
    !character.is_control()
        && !matches!(character, '\u{061c}' | '\u{200b}'..='\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{206f}' | '\u{feff}')
}

/// Drops a `Title:`-style lead-in, when there is one.
///
/// Only the labels a model actually writes, and only when there is a name after
/// them: `fix: the footer` is a title with a colon in it and keeps its verb,
/// while `Title:` alone is a label with nothing under it -- and a name that is
/// only a label is no name, so what is left is refused.
fn strip_label(text: &str) -> &str {
    const LABELS: [&str; 5] = ["title", "name", "session", "label", "conversation"];
    let Some((head, rest)) = text.split_once(':') else {
        return text;
    };
    let head = head.trim();
    if LABELS.iter().any(|label| head.eq_ignore_ascii_case(label)) {
        rest.trim()
    } else {
        text
    }
}

/// Truncates on a character boundary, at a word when one is close enough.
fn truncate_chars(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let cut: String = text.chars().take(limit).collect();
    // Backing up to the last space keeps a word whole; only a title with no
    // space in the last few characters is cut mid-word.
    match cut.rfind(' ') {
        Some(at) if at > limit / 2 => cut[..at].trim_end().to_owned(),
        _ => cut.trim_end().to_owned(),
    }
}

impl Service {
    /// Asks for a title for `first_message`, and gives the session one.
    ///
    /// Best effort by contract: this runs beside the turn the person just
    /// started, and a title that cannot be had is not a reason to disturb it.
    /// Every failure — a provider that refuses, a model that answers with
    /// nothing, a session that is already gone — is reported to the caller as an
    /// `Err` and logged, and the session keeps the name it had.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError`] when the titling model cannot be resolved, the
    /// request fails, or the reply contains no usable title.
    pub async fn title_session(
        &self,
        key: &norte_code_protocol::SessionKey,
        first_message: &str,
    ) -> Result<String, ServiceError> {
        if !self.config().titling.enabled {
            return Err(ServiceError::Titling("titling is disabled".to_owned()));
        }
        let first_message = first_message.trim();
        if first_message.is_empty() {
            return Err(ServiceError::Titling(
                "there is no message to name the session after".to_owned(),
            ));
        }
        // The session has to exist before it can be renamed, and the caller may
        // have moved on: a title that arrives after the session was deleted is
        // not an error worth raising, it is a name with nowhere to go.
        let (_, session_spec, _) = self.require_record(key)?;
        let spec = self.titling_spec(&session_spec)?;
        let engine = self.engine(&spec)?;
        let scratch = engine.create().await?;
        // No tools and no reasoning: the question is "what is this called", and
        // a tool call or a chain of thought is neither a cheaper nor a better
        // answer to it. The instruction is the whole history.
        let outcome = engine
            .turn(scratch.id)
            .input(Message::user(format!(
                "{TITLE_PROMPT}\n\n---\n{first_message}"
            )))
            .reasoning(ReasoningRequest::Disabled)
            .tools(std::iter::empty::<norte::ToolName>())
            .max_output_tokens(self.config().titling.max_output)
            .start()
            .finish()
            .await?;
        let message = outcome
            .message
            .filter(|message| message.role == Role::Assistant)
            .ok_or_else(|| {
                ServiceError::Titling("the title request committed no answer".to_owned())
            })?;
        let title = sanitise_title(&message.text()).ok_or_else(|| {
            ServiceError::Titling("the model produced no usable title".to_owned())
        })?;
        self.rename(key, &title).await?;
        Ok(title)
    }

    /// The model a title is asked of.
    ///
    /// `titling.model` when set, the session's own otherwise. The resolution is
    /// the one a model switch uses, so a titler can be a provider the catalogue
    /// does not list, and a configured one that cannot be built is the error the
    /// caller reports rather than a silent fall back to a model nobody chose.
    fn titling_spec(&self, session: &ModelSpec) -> Result<ModelSpec, ServiceError> {
        match &self.config().titling.model {
            Some(reference) => {
                Service::default_spec_for(&self.lock_config(), &self.catalog(), reference)
            }
            None => Ok(session.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_title_keeps_the_words_and_drops_the_wrapping() {
        assert_eq!(
            sanitise_title("fix the footer overflow"),
            Some("fix the footer overflow".to_owned())
        );
        // A model told not to wrap a title sometimes wraps it anyway.
        assert_eq!(
            sanitise_title("\"fix the footer overflow\""),
            Some("fix the footer overflow".to_owned())
        );
        assert_eq!(
            sanitise_title("  fix the footer overflow.  "),
            Some("fix the footer overflow".to_owned())
        );
        // A lead-in label is not part of the name.
        assert_eq!(
            sanitise_title("Title: fix the footer"),
            Some("fix the footer".to_owned())
        );
    }

    #[test]
    fn only_the_first_line_is_the_title() {
        // A model that answers the question and then explains itself: the
        // explanation is not a name, and a list row is one line.
        assert_eq!(
            sanitise_title("fix the footer overflow\n\nI chose this because the message..."),
            Some("fix the footer overflow".to_owned())
        );
        assert_eq!(
            sanitise_title("fix the footer\nand the second thing"),
            Some("fix the footer".to_owned())
        );
    }

    #[test]
    fn a_title_cannot_carry_terminal_controls_into_a_list() {
        // The title reaches a footer and a startup row, and a model's text is
        // not trusted with either.
        assert_eq!(
            sanitise_title("fix\u{1b}[2J the footer"),
            Some("fix[2J the footer".to_owned())
        );
        assert_eq!(
            sanitise_title("fix\u{202e} the footer"),
            Some("fix the footer".to_owned()),
            "a bidi override is not a control to Rust, and it rewrites the reading \
             order of everything after it: in a list of sessions that is a row \
             that reads as a different session than it opens"
        );
        // A tab is separation and folds into a space; the newline ends the
        // title, because a reply with two lines answered with more than a name.
        assert_eq!(sanitise_title("a\tb"), Some("a b".to_owned()));
        assert_eq!(sanitise_title("a\tb\nc"), Some("a b".to_owned()));
    }

    #[test]
    fn a_reply_that_is_not_a_name_is_no_title_at_all() {
        // Nothing usable, so the session keeps its key: a row named `...` or `"`
        // is worse than a row named by its key, because it claims to say
        // something.
        assert_eq!(sanitise_title(""), None);
        assert_eq!(sanitise_title("   \n  "), None);
        assert_eq!(sanitise_title("\"\""), None);
        assert_eq!(sanitise_title("..."), None);
        assert_eq!(sanitise_title("12345"), None);
        assert_eq!(
            sanitise_title("Title:"),
            None,
            "a label with nothing after it"
        );
    }

    #[test]
    fn a_title_longer_than_a_row_is_cut_at_a_word() {
        let long = "fix the footer overflow when the terminal is narrower than \
                    ninety columns and the session key is long";
        let title = sanitise_title(long).expect("a title");
        assert!(
            title.chars().count() <= MAX_TITLE_CHARS,
            "the title fits: {title:?}"
        );
        assert!(
            long.starts_with(title.as_str()),
            "and it is a prefix of what the model said: {title:?}"
        );
        assert!(
            !title.ends_with(' '),
            "a cut title does not end in the space it was cut at: {title:?}"
        );
        // A single word longer than the limit is still cut, rather than kept
        // whole: the row is what the limit protects.
        let one_word = "a".repeat(200);
        let cut = sanitise_title(&one_word).expect("a title");
        assert_eq!(cut.chars().count(), MAX_TITLE_CHARS);
    }
}
