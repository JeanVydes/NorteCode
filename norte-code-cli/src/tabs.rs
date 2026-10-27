//! The tabs inside one session, and which one is showing.
//!
//! The client shows a handful of things about one session: what it has spent,
//! the task list, the tools the model can reach and the MCP servers behind some
//! of them. A tab is a named view of one of them, and the row above the input
//! says where the person is and what else there is.
//!
//! A tab carries a one-row summary, because that is the whole of the room the
//! footer has: the list itself is a command (`/tools`, `/mcp`) that prints into
//! the transcript, where there is space for it and it can be scrolled back to.
//! A tab whose summary does not fit says so by counting what it left out, the
//! way every other bounded line in this client does.
//!
//! The startup information is not a tab: it is thirty lines of panels and the
//! row has one, so it is `/debug`, which prints it into the transcript where
//! there is room for it.
//!
//! The state is small and lives here rather than in the input editor, because
//! both the editor (which draws the footer) and the loop (which decides what a
//! key does) need it, and neither owns the other.

use std::fmt;

/// What a session tab shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum SessionTab {
    /// What the session has spent and what it holds, which is where a client
    /// starts: it is the line the footer already carried, given a name so the
    /// other tabs have somewhere to sit beside it.
    Stats,
    /// The model's task list.
    Todo,
    /// Every tool the model can reach, and how a call to it is decided.
    Tools,
    /// The MCP servers, connected or not.
    Mcp,
}

impl SessionTab {
    /// Every tab, in the order the footer lists them.
    ///
    /// The order is the order of the numbers: `Ctrl-1` is the first, and the
    /// row reads left to right, so the key and the position agree.
    pub(crate) const ALL: [SessionTab; 4] = [
        SessionTab::Stats,
        SessionTab::Todo,
        SessionTab::Tools,
        SessionTab::Mcp,
    ];

    /// The bare name, for the footer and the tests.
    pub(crate) fn name(self) -> &'static str {
        match self {
            SessionTab::Stats => "stats",
            SessionTab::Todo => "todo",
            SessionTab::Tools => "tools",
            SessionTab::Mcp => "mcp",
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
    ///
    /// `chat` was this tab's name before it was `stats`, and it is still
    /// accepted: the conversation is what the stats are about, so the old name
    /// is not wrong, only less precise.
    pub(crate) fn named(name: &str) -> Option<Self> {
        let name = name.trim();
        if name.eq_ignore_ascii_case("chat") {
            return Some(SessionTab::Stats);
        }
        Self::ALL
            .iter()
            .copied()
            .find(|tab| tab.name().eq_ignore_ascii_case(name))
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
        assert_eq!(SessionTab::Stats.next(), SessionTab::Todo);
        assert_eq!(SessionTab::Tools.next(), SessionTab::Mcp);
        assert_eq!(
            SessionTab::Mcp.next(),
            SessionTab::Stats,
            "the last wraps to the first"
        );
        assert_eq!(SessionTab::Stats.previous(), SessionTab::Mcp);
        assert_eq!(SessionTab::Mcp.previous(), SessionTab::Tools);
        // Every tab is reachable from every other by cycling, which is what
        // makes `Ctrl-T` enough on its own.
        let mut seen = Vec::new();
        let mut tab = SessionTab::Stats;
        for _ in 0..SessionTab::ALL.len() {
            seen.push(tab);
            tab = tab.next();
        }
        assert_eq!(seen, SessionTab::ALL.to_vec());
    }

    #[test]
    fn a_tab_is_named_by_a_number_or_a_word() {
        assert_eq!(SessionTab::numbered(1), Some(SessionTab::Stats));
        assert_eq!(SessionTab::numbered(2), Some(SessionTab::Todo));
        assert_eq!(SessionTab::numbered(3), Some(SessionTab::Tools));
        assert_eq!(SessionTab::numbered(4), Some(SessionTab::Mcp));
        assert_eq!(SessionTab::numbered(0), None, "numbers count from one");
        assert_eq!(SessionTab::numbered(5), None, "there is no fifth tab");

        assert_eq!(SessionTab::named("todo"), Some(SessionTab::Todo));
        assert_eq!(SessionTab::named("tools"), Some(SessionTab::Tools));
        assert_eq!(SessionTab::named("mcp"), Some(SessionTab::Mcp));
        assert_eq!(SessionTab::named(" Stats "), Some(SessionTab::Stats));
        // The name this tab had before it was `stats`.
        assert_eq!(SessionTab::named("chat"), Some(SessionTab::Stats));
        assert_eq!(SessionTab::named("CHAT"), Some(SessionTab::Stats));
        assert_eq!(SessionTab::named("nothing"), None);
        // `debug` was a tab and is a command now: asking for it by name is an
        // error that names the tabs that do exist.
        assert_eq!(SessionTab::named("debug"), None);
    }

    #[test]
    fn every_tab_names_itself_and_no_two_share_a_name() {
        // The footer lists them by name and `/tab <name>` finds them that way,
        // so a duplicate name would make one of the two unreachable.
        let mut names: Vec<&str> = SessionTab::ALL.iter().map(|tab| tab.name()).collect();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count, "every tab has its own name");
    }
}
