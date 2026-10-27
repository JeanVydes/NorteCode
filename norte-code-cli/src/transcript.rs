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
//! # Ink, not escapes
//!
//! A row's colour is a [`Line::ink`], and never an escape sequence inside its
//! text. The rows used to be built with the string painters — `style.green(text)`
//! and friends — and those wrap text in raw SGR codes, which is right for a
//! printer writing to a terminal and wrong for a frame that draws text: the
//! frame put the sequences on screen as the characters they are. A diff came out
//! as `\u{1b}[31m- old line\u{1b}[0m` where a red row was meant.
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

/// The ink one row is written in, when the entry's own ink is not what it wants.
///
/// A diff is the reason this exists: inside one tool result, a context line, a
/// removal and an addition are three different things, and the entry's kind
/// cannot say which is which. A row carries its ink rather than a string of
/// escape codes, because the frame draws text and would put the codes on screen
/// as the characters they are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ink {
    /// Quiet, for context that is read past rather than read.
    Faint,
    /// A step louder than faint: a label or a number.
    Dim,
    /// A section name inside a result.
    Cyan,
    /// What arrived.
    Green,
    /// What left.
    Red,
    /// A bound that was reached, or something that wants attention.
    Yellow,
}

/// One row of the conversation.
///
/// The text is what arrived and nothing else: no wrap, no escape sequence, no
/// marker. Everything the view needs to decide how to draw it is in the ink.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    /// The row's text, as it arrived.
    pub text: String,
    /// The ink to draw it in, when it is not the entry's own.
    pub ink: Option<Ink>,
}

impl Line {
    /// A row in the entry's own ink.
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ink: None,
        }
    }

    /// A row in an ink of its own.
    #[must_use]
    pub fn inked(ink: Ink, text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ink: Some(ink),
        }
    }
}

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
    /// The rows, as they arrived.
    ///
    /// Never wrapped: the width is the view's business, and a wrap kept here
    /// would be a wrap for a width the terminal may no longer have.
    pub lines: Vec<Line>,
    /// How many lines the bound dropped, so the view can say so.
    pub dropped: usize,
    /// When it started, for an entry that is still running.
    ///
    /// A tool call is announced before it finishes, and its row counts while it
    /// runs, so the instant it began travels with it. `None` for an entry that
    /// counts nothing: a message has no duration, and a timer that never stops
    /// leaves a number that means nothing.
    pub started: Option<std::time::Instant>,
    /// The call this entry reports on, for a row that is rewritten in place.
    ///
    /// A tool used to be two rows — one saying it began and one saying how it
    /// ended — so a suite of ten calls put twenty lines in the conversation and
    /// the reader had to pair them up. One call is one row: it says what is
    /// running while it runs, and the same row gains its outcome when it ends.
    /// The id is what makes that possible: it is the only thing that says which
    /// row an ending belongs to.
    pub call_id: Option<String>,
    /// Whether the view draws a frame around this entry's rows.
    ///
    /// A tool's output is one bounded thing, and a frame is what says so. The
    /// frame is drawn by the view and not stored here: it is chrome, and a box
    /// whose edges were rows of the conversation would be a box a person cannot
    /// select the inside of without taking the edges with it.
    pub boxed: bool,
    /// What the frame says on its top edge, when it says anything.
    pub title: Option<String>,
}

impl Entry {
    /// An entry with no text.
    #[must_use]
    pub fn new(kind: Kind) -> Self {
        Self {
            kind,
            lines: Vec::new(),
            dropped: 0,
            started: None,
            call_id: None,
            boxed: false,
            title: None,
        }
    }

    /// An entry that is one line.
    #[must_use]
    pub fn line(kind: Kind, text: impl Into<String>) -> Self {
        let mut entry = Self::new(kind);
        entry.push_line(text);
        entry
    }

    /// An empty entry the view draws a frame around.
    #[must_use]
    pub fn bordered(kind: Kind, title: impl Into<String>) -> Self {
        let mut entry = Self::new(kind);
        entry.boxed = true;
        entry.title = Some(title.into());
        entry
    }

    /// Adds one line, cutting it and counting what is left out.
    pub fn push_line(&mut self, text: impl Into<String>) {
        let text = clip_line(&text.into());
        self.lines.push(Line::new(text));
        self.trim();
    }

    /// Adds one line in an ink of its own, cutting it like any other.
    pub fn push_inked(&mut self, ink: Ink, text: impl Into<String>) {
        let text = clip_line(&text.into());
        self.lines.push(Line::inked(ink, text));
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
            writeln!(f, "{}", line.text)?;
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
        // An entry that is only a frame and a title is still something to draw:
        // an empty box is what a call with nothing to show looks like, and
        // dropping it would leave the announcement with nothing under it.
        if !entry.is_empty() || entry.boxed {
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
                Some(line) => line.text.push_str(&clip_line(&first)),
                None => last.push_line(first),
            }
        }
        for part in parts {
            last.push_line(part.to_owned());
        }
        last.trim();
    }

    /// The newest entry reporting on a call, to rewrite it in place.
    ///
    /// It is the row a tool announced itself on. An ending does not add a row
    /// of its own: it edits this one, which is what makes one call one line.
    pub fn call_mut(&mut self, call_id: &str) -> Option<&mut Entry> {
        self.entries
            .iter_mut()
            .rev()
            .find(|entry| entry.call_id.as_deref() == Some(call_id))
    }

    /// Ends the open entry, so the next line starts a new one.
    pub fn close(&mut self) {
        self.open = false;
    }

    /// The newest entry, to change how the view draws it.
    ///
    /// A streamed block is appended to as it arrives, and whether it is one
    /// bounded thing — a thought, a command's output — is only known when it
    /// ends. The frame is drawn by the view and set here, because the entry is
    /// the thing the view draws.
    pub fn last_mut(&mut self) -> Option<&mut Entry> {
        self.entries.last_mut()
    }
    /// How long the last entry has been running, when it is running.
    ///
    /// `None` means it counts nothing: an entry with no start, or one that
    /// finished and had its timer taken. The view asks this to draw the number
    /// beside a call that is still out.
    ///
    /// The frame asks it too: a call that is out is a clock the view has to
    /// keep redrawing, and one that has finished is a screen that can sit
    /// still until the next key.
    #[must_use]
    pub fn running_for(&self) -> Option<std::time::Duration> {
        self.entries.last()?.started.map(|at| at.elapsed())
    }

    /// Stops the last entry's timer, and says how long it ran.
    ///
    /// What the row shows once a call ends is the duration the service
    /// reported; the clock here is only for the wait before that arrives.
    pub fn stop_timer(&mut self) -> Option<std::time::Duration> {
        self.entries
            .last_mut()?
            .started
            .take()
            .map(|at| at.elapsed())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The text of an entry's rows, which is what most of these are about.
    fn texts(entry: &Entry) -> Vec<String> {
        entry.lines.iter().map(|line| line.text.clone()).collect()
    }

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
        assert_eq!(texts(&transcript.entries()[0]), ["Hello world"]);
    }

    #[test]
    fn a_newline_in_a_token_starts_a_line_and_not_an_entry() {
        let mut transcript = Transcript::new();
        transcript.append(Kind::Answer, "one\ntwo");
        transcript.append(Kind::Answer, "\nthree");
        assert_eq!(transcript.len(), 1, "{transcript:?}");
        assert_eq!(texts(&transcript.entries()[0]), ["one", "two", "three"]);
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
        let line = &entry.lines[0].text;
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
            entry.lines[0].text,
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
        assert_eq!(entry.lines[0].text, "a    b");
    }

    #[test]
    fn a_running_entry_counts_and_a_finished_one_stops() {
        // The header of a tool call shows the call running, so its clock has to
        // be there and has to stop: a count that kept going beside a finished
        // call is a number that lies.
        let mut transcript = Transcript::new();
        let mut entry = Entry::line(Kind::ToolStarted, "→ shell #abc");
        entry.started = Some(std::time::Instant::now());
        transcript.push(entry);
        assert!(
            transcript.running_for().is_some(),
            "a call that is out is counting"
        );
        let measured = transcript.stop_timer();
        assert!(measured.is_some(), "and it reports what it measured");
        assert!(
            transcript.running_for().is_none(),
            "and it stops counting when it ends"
        );
    }

    #[test]
    fn an_entry_that_counts_nothing_says_so() {
        // A message has no duration: a timer on every entry would put a number
        // beside things that never run.
        let mut transcript = Transcript::new();
        transcript.push(Entry::line(Kind::Said, "hello"));
        assert!(transcript.running_for().is_none());
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
        assert_eq!(texts(&entry), ["one", "two", "three"]);
    }

    #[test]
    fn an_entry_carries_the_ink_of_each_of_its_rows() {
        // A diff is the reason this exists: inside one tool result a removal and
        // an addition are two different things, and the entry's kind cannot say
        // which is which. The ink is data, not an escape sequence in the text:
        // a frame draws text, and a code kept in it would be drawn as characters.
        let mut entry = Entry::new(Kind::ToolOutput);
        entry.push_inked(Ink::Red, "- old line");
        entry.push_line("  context");
        entry.push_inked(Ink::Green, "+ new line");
        assert_eq!(entry.lines[0].ink, Some(Ink::Red));
        assert_eq!(entry.lines[1].ink, None, "plain rows keep the entry's ink");
        assert_eq!(entry.lines[2].ink, Some(Ink::Green));
        assert_eq!(entry.lines[2].text, "+ new line");
    }

    #[test]
    fn no_row_of_the_conversation_carries_an_escape_sequence() {
        // The bug this whole design exists to prevent: the rows were built with
        // the string painters, those wrap text in raw SGR codes, and the frame
        // put the codes on screen as the characters they are.
        let mut entry = Entry::new(Kind::ToolOutput);
        entry.push_inked(Ink::Red, "- old");
        entry.push_inked(Ink::Green, "+ new");
        for line in &entry.lines {
            assert!(
                !line.text.contains('\u{1b}'),
                "a row carries ink, never an escape: {:?}",
                line.text
            );
        }
    }

    #[test]
    fn an_entry_the_view_frames_is_kept_even_with_no_rows() {
        // A call that produced nothing still happened, and the frame is what
        // says so. `push` used to drop an entry with no lines, which would leave
        // the announcement with nothing under it.
        let mut transcript = Transcript::new();
        transcript.push(Entry::bordered(Kind::ToolOutput, "shell #abc"));
        assert_eq!(transcript.len(), 1);
        assert!(transcript.entries()[0].boxed);
        assert_eq!(transcript.entries()[0].title.as_deref(), Some("shell #abc"));
        // And a plain entry with no rows is still nothing to draw.
        transcript.push(Entry::new(Kind::Notice));
        assert_eq!(transcript.len(), 1);
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
