//! The conversation, as the client holds it.
//!
//! The renderer used to print each event as it arrived and keep nothing: a
//! tool's output, a diff, a message — all of them went to the terminal and were
//! forgotten. That works until the interface has to do anything but append, and
//! then it does not work at all: a resize cannot re-wrap what is no longer
//! there, scrolling cannot go back, and nothing can say how much it dropped.
//!
//! So the conversation is data. An [`Entry`] is one thing that was said or done,
//! in the order it happened, holding the text **as it arrived** rather than as
//! it was drawn: a wrapped line is only correct at the width it was wrapped for,
//! and a wrapped line re-wrapped is words broken in the wrong places.
//!
//! # Bounds
//!
//! Two of them, and they answer different questions:
//!
//! - A **line** is capped by [`MAX_LINE_CHARS`], because one line of ten
//!   thousand characters is one row in the data and two hundred on screen, and
//!   a command that prints one is a command that would take the whole view.
//! - An **entry** is capped by [`MAX_ENTRY_LINES`], counting what it left out.
//!   A tool that prints a thousand lines is one entry, and an interface that
//!   keeps all thousand is an interface that runs out of memory in the middle of
//!   a session.
//!
//! Both cuts are recorded rather than silent. That is the rule the rest of the
//! client already follows: a bounded list that hides its own bound is worse than
//! a short one that admits it.

use std::fmt;

/// The most characters one line keeps.
///
/// It is the row's own bound and not the entry's: a line longer than this is a
/// line no view shows in full at any width a terminal has.
pub const MAX_LINE_CHARS: usize = 4_096;

/// The most lines one entry keeps.
///
/// Past it the oldest lines are dropped and counted, which is what the tool
/// output box already did while it was being printed.
pub const MAX_ENTRY_LINES: usize = 1_000;

/// The most lines one thought keeps.
///
/// A thought is the one entry that grows without bound: a model reasoning about
/// a hard problem writes thousands of lines, and none of them are the answer.
/// It is kept short so the conversation stays the thing on screen — past this
/// the oldest lines go and the count says so, which is what every other bound
/// in this client does.
pub const MAX_THOUGHT_LINES: usize = 12;

/// What kind of thing an entry is.
///
/// It decides how the entry is drawn and nothing else; two entries of the same
/// kind are drawn by the same code, which is what keeps a diff looking like a
/// diff wherever it came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// What the human typed.
    Said,
    /// The model's answer.
    Answer,
    /// The model's reasoning, shown in a gutter.
    Reasoning,
    /// A tool announcement, with what it was asked.
    ToolStarted,
    /// What a tool produced.
    ToolOutput,
    /// A line of progress or a notice: waiting, retrying, compacting.
    Notice,
    /// Something went wrong, from the client or from the provider.
    Failure,
}

/// One thing that happened, in the order it happened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// What kind of thing it is.
    pub kind: Kind,
    /// The lines, as they arrived.
    ///
    /// Never wrapped: the width is the view's business, and a wrap kept here
    /// would be a wrap for a width the terminal may no longer have.
    pub lines: Vec<String>,
    /// How many lines the bound dropped, so the view can say so.
    pub dropped: usize,
}

impl Entry {
    /// An entry with no text.
    #[must_use]
    pub fn new(kind: Kind) -> Self {
        Self {
            kind,
            lines: Vec::new(),
            dropped: 0,
        }
    }

    /// An entry that is one line.
    #[must_use]
    pub fn line(kind: Kind, text: impl Into<String>) -> Self {
        let mut entry = Self::new(kind);
        entry.push_line(text);
        entry
    }

    /// Adds one line, cutting it and counting what is left out.
    pub fn push_line(&mut self, text: impl Into<String>) {
        let text = text.into();
        let text = clip_line(&text);
        self.lines.push(text);
        self.trim();
    }

    /// Adds text that may be several lines.
    pub fn push_text(&mut self, text: &str) {
        for line in text.split('\n') {
            self.push_line(line);
        }
    }

    /// Whether the entry has nothing to show.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Drops the oldest lines past the bound, and counts them.
    ///
    /// The newest are the ones worth keeping: a tool's output is read from its
    /// end, and what a person scrolls to is what just happened.
    fn trim(&mut self) {
        let bound = match self.kind {
            Kind::Reasoning => MAX_THOUGHT_LINES,
            _ => MAX_ENTRY_LINES,
        };
        if self.lines.len() <= bound {
            return;
        }
        let excess = self.lines.len() - bound;
        self.lines.drain(..excess);
        self.dropped += excess;
    }
}

impl fmt::Display for Entry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for line in &self.lines {
            writeln!(f, "{line}")?;
        }
        Ok(())
    }
}

/// Cuts a line to the bound, saying nothing about the cut.
///
/// The count is not kept per line on purpose: a line cut in the middle is
/// visibly cut — it ends in an ellipsis — and a number per line would be more
/// screen than the line it belongs to.
fn clip_line(text: &str) -> String {
    // Tabs become spaces here because the width of a tab depends on the column
    // it starts at, and a line measured before the view knows its column would
    // be measured wrong.
    let escaped = text.replace('\t', "    ");
    if escaped.chars().count() <= MAX_LINE_CHARS {
        return escaped;
    }
    let mut cut: String = escaped.chars().take(MAX_LINE_CHARS - 1).collect();
    cut.push('…');
    cut
}

/// The conversation, oldest first.
#[derive(Clone, Debug, Default)]
pub struct Transcript {
    entries: Vec<Entry>,
    /// Whether the newest entry may be added to instead of starting one.
    ///
    /// A streamed answer arrives one token at a time, and a thousand tokens are
    /// one entry: appending to the last is what makes the answer a paragraph
    /// rather than a thousand lines that happen to be adjacent.
    open: bool,
}

impl Transcript {
    /// An empty conversation.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Every entry, oldest first.
    #[must_use]
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// How many entries there are.
    #[cfg(test)]
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing has happened.
    #[cfg(test)]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Forgets everything, keeping the allocation.
    ///
    /// A resumed session replaces the conversation it was showing with the one
    /// it replays, and rebuilding the vec for that would be a copy of every
    /// entry to throw the old ones away.
    #[cfg(test)]
    pub fn clear(&mut self) {
        self.entries.clear();
        self.open = false;
    }

    /// Adds a whole entry.
    pub fn push(&mut self, entry: Entry) {
        self.open = false;
        if !entry.is_empty() {
            self.entries.push(entry);
        }
    }

    /// Appends text to the open entry, or starts one.
    ///
    /// It is what a token of an answer does: the text is cut on the newlines so
    /// the entry holds lines, and the last of them is the one still growing.
    pub fn append(&mut self, kind: Kind, text: &str) {
        let same = self.open && self.entries.last().is_some_and(|last| last.kind == kind);
        if !same {
            self.entries.push(Entry::new(kind));
            self.open = true;
        }
        let Some(last) = self.entries.last_mut() else {
            return;
        };
        // A token need not end on a line: what is before the last newline is a
        // line, and what is after it belongs to the line being written.
        let mut parts = text.split('\n');
        if let Some(first) = parts.next() {
            let first = first.to_owned();
            match last.lines.last_mut() {
                Some(line) => line.push_str(&clip_line(&first)),
                None => last.push_line(first),
            }
        }
        for part in parts {
            last.push_line(part.to_owned());
        }
        last.trim();
    }

    /// Ends the open entry, so the next line starts a new one.
    pub fn close(&mut self) {
        self.open = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_streamed_answer_is_one_entry_and_not_one_per_token() {
        // The whole reason `append` exists: a token at a time would otherwise
        // be a line at a time, and an answer of five hundred tokens would be
        // five hundred entries to scroll through.
        let mut transcript = Transcript::new();
        for token in ["Hel", "lo", " world"] {
            transcript.append(Kind::Answer, token);
        }
        assert_eq!(transcript.len(), 1, "{transcript:?}");
        assert_eq!(transcript.entries()[0].lines, ["Hello world"]);
    }

    #[test]
    fn a_newline_in_a_token_starts_a_line_and_not_an_entry() {
        let mut transcript = Transcript::new();
        transcript.append(Kind::Answer, "one\ntwo");
        transcript.append(Kind::Answer, "\nthree");
        assert_eq!(transcript.len(), 1, "{transcript:?}");
        assert_eq!(transcript.entries()[0].lines, ["one", "two", "three"]);
    }

    #[test]
    fn a_different_kind_starts_its_own_entry() {
        // An answer that follows a tool's output is a new thing said, and
        // joining them would make the transcript read as if it were not.
        let mut transcript = Transcript::new();
        transcript.append(Kind::Reasoning, "thinking");
        transcript.append(Kind::Answer, "the answer");
        assert_eq!(transcript.len(), 2, "{transcript:?}");
        assert_eq!(transcript.entries()[0].kind, Kind::Reasoning);
        assert_eq!(transcript.entries()[1].kind, Kind::Answer);
    }

    #[test]
    fn closing_ends_the_entry_so_the_next_token_starts_another() {
        let mut transcript = Transcript::new();
        transcript.append(Kind::Answer, "first");
        transcript.close();
        transcript.append(Kind::Answer, "second");
        assert_eq!(transcript.len(), 2, "{transcript:?}");
    }

    #[test]
    fn a_line_past_the_bound_is_cut_and_says_so_in_the_text() {
        // A line is cut visibly rather than counted: the number would take more
        // room than the line it belongs to.
        let mut entry = Entry::new(Kind::ToolOutput);
        entry.push_line("x".repeat(MAX_LINE_CHARS + 100));
        let line = &entry.lines[0];
        assert_eq!(line.chars().count(), MAX_LINE_CHARS);
        assert!(line.ends_with('…'), "the cut is visible");
    }

    #[test]
    fn an_entry_past_the_bound_drops_the_oldest_and_counts_them() {
        // A tool that prints a thousand lines is one entry, and the newest are
        // the ones worth keeping: output is read from its end.
        let mut entry = Entry::new(Kind::ToolOutput);
        for index in 0..MAX_ENTRY_LINES + 10 {
            entry.push_line(format!("line {index}"));
        }
        assert_eq!(entry.lines.len(), MAX_ENTRY_LINES);
        assert_eq!(entry.dropped, 10);
        assert_eq!(
            entry.lines[0],
            format!("line {}", 10),
            "the oldest went first"
        );
    }

    #[test]
    fn a_tab_is_measured_as_the_spaces_it_becomes() {
        // A tab's width depends on the column it starts at, and this does not
        // know the column: it is written as spaces so the count is honest.
        let mut entry = Entry::new(Kind::Answer);
        entry.push_line("a\tb");
        assert_eq!(entry.lines[0], "a    b");
    }

    #[test]
    fn an_empty_entry_is_not_added() {
        // A notice that printed nothing is not a thing that happened, and an
        // entry with no lines would be a blank row with nothing to explain it.
        let mut transcript = Transcript::new();
        transcript.push(Entry::new(Kind::Notice));
        assert!(transcript.is_empty());
    }

    #[test]
    fn several_lines_arrive_as_several_lines() {
        let mut entry = Entry::new(Kind::ToolOutput);
        entry.push_text("one\ntwo\nthree");
        assert_eq!(entry.lines, ["one", "two", "three"]);
    }

    #[test]
    fn the_transcript_keeps_the_order_things_happened_in() {
        let mut transcript = Transcript::new();
        transcript.push(Entry::line(Kind::Said, "do the thing"));
        transcript.append(Kind::Answer, "doing it");
        transcript.close();
        transcript.push(Entry::line(Kind::ToolStarted, "read"));
        let kinds: Vec<Kind> = transcript
            .entries()
            .iter()
            .map(|entry| entry.kind)
            .collect();
        assert_eq!(
            kinds,
            [Kind::Said, Kind::Answer, Kind::ToolStarted],
            "the order is the conversation"
        );
    }
}
