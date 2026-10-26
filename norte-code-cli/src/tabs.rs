//! The tabs inside one session, and which one is showing.
//!
//! The client shows several things about one session — the conversation, the
//! task list, and the debug information that used to be dumped before the first
//! prompt. They used to be one stream: debug information was printed at startup
//! and scrolled away, and the task list shared the pinned row with the session
//! status. A tab is a named view of one of them, and the row above the input
//! says where the person is and what else there is.
//!
//! The state is small and lives here rather than in the input editor, because
//! both the editor (which draws the footer) and the loop (which decides what a
//! key does) need it, and neither owns the other.

use std::fmt;

/// What a session tab shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum SessionTab {
    /// The conversation, which is where a client starts.
    Chat,
    /// The model's task list.
    Todo,
    /// What the process learned at startup: the model, the paths, the servers.
    Debug,
}

impl SessionTab {
    /// Every tab, in the order the footer lists them.
    pub(crate) const ALL: [SessionTab; 3] = [SessionTab::Chat, SessionTab::Todo, SessionTab::Debug];

    /// The bare name, for the footer and the tests.
    pub(crate) fn name(self) -> &'static str {
        match self {
            SessionTab::Chat => "chat",
            SessionTab::Todo => "todo",
            SessionTab::Debug => "debug",
        }
    }

    /// The tab after this one, wrapping at the end.
    pub(crate) fn next(self) -> Self {
        let at = Self::ALL.iter().position(|tab| *tab == self).unwrap_or(0);
        Self::ALL[(at + 1) % Self::ALL.len()]
    }

    /// The tab before this one, wrapping at the start.
    pub(crate) fn previous(self) -> Self {
        let at = Self::ALL.iter().position(|tab| *tab == self).unwrap_or(0);
        Self::ALL[(at + Self::ALL.len() - 1) % Self::ALL.len()]
    }

    /// The tab a number names, counting from one, as `/tab 2` and `Ctrl-2` do.
    pub(crate) fn numbered(number: usize) -> Option<Self> {
        Self::ALL.get(number.checked_sub(1)?).copied()
    }

    /// The tab a name names, as `/tab todo` does.
    pub(crate) fn named(name: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|tab| tab.name().eq_ignore_ascii_case(name.trim()))
    }
}

impl fmt::Display for SessionTab {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tabs_cycle_both_ways_and_wrap() {
        assert_eq!(SessionTab::Chat.next(), SessionTab::Todo);
        assert_eq!(SessionTab::Todo.next(), SessionTab::Debug);
        assert_eq!(
            SessionTab::Debug.next(),
            SessionTab::Chat,
            "the last wraps to the first"
        );
        assert_eq!(SessionTab::Chat.previous(), SessionTab::Debug);
        assert_eq!(SessionTab::Debug.previous(), SessionTab::Todo);
        // Every tab is reachable from every other by cycling, which is what
        // makes `Ctrl-T` enough on its own.
        let mut seen = Vec::new();
        let mut tab = SessionTab::Chat;
        for _ in 0..SessionTab::ALL.len() {
            seen.push(tab);
            tab = tab.next();
        }
        assert_eq!(seen, SessionTab::ALL.to_vec());
    }

    #[test]
    fn a_tab_is_named_by_a_number_or_a_word() {
        assert_eq!(SessionTab::numbered(1), Some(SessionTab::Chat));
        assert_eq!(SessionTab::numbered(3), Some(SessionTab::Debug));
        assert_eq!(SessionTab::numbered(0), None, "numbers count from one");
        assert_eq!(SessionTab::numbered(4), None, "there is no fourth tab");

        assert_eq!(SessionTab::named("todo"), Some(SessionTab::Todo));
        assert_eq!(SessionTab::named(" Debug "), Some(SessionTab::Debug));
        assert_eq!(SessionTab::named("nothing"), None);
    }
}
