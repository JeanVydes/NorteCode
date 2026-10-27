//! A menu a person walks with the arrow keys.
//!
//! The startup flow asks two questions — which session to open, which model to
//! use — and both are lists with a couple of entries that matter and a dozen
//! that do not. A numbered prompt makes the person read the list, count, and
//! type; a cursor lets them walk it and press Enter, which is what a terminal
//! has keys for.
//!
//! The list and its cursor are a plain value with no terminal in them: what a
//! key means is a method, and it is tested without a tty. Only [`run`] touches
//! the terminal, and it runs *before* the input editor exists — the editor takes
//! the terminal over with a scroll region and a pinned footer, and a menu that
//! borrowed it would have to hand it back before the session it is choosing for
//! has even started.
//!
//! Typing filters. Every one of these menus already accepted a typed name, and
//! the cursor keeps that: a name narrows the list rather than having to match an
//! entry exactly, so a provider or a session key that fell off the end of a
//! bounded list is still two keystrokes away.

use crate::style::Style;

/// How many rows the menu shows at once.
///
/// A terminal shows more, but the answer comes from reading a handful: past this
/// the list scrolls around the cursor, and the count that did not fit is printed
/// beside the last row, because a bounded list that hides its own bound is worse
/// than a short one that admits it.
pub(crate) const VISIBLE_ROWS: usize = 12;

/// One entry of a menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Item {
    /// What the row shows.
    pub(crate) label: String,
    /// A word to the right of the label, when there is one worth saying.
    pub(crate) note: Option<String>,
    /// Whether the row is one of the fixed entries, which are drawn bold.
    pub(crate) fixed: bool,
    /// What filtering looks at besides the label.
    ///
    /// A session is reachable by the key in its label and by the title beside
    /// it, and both are things a person types. This is the text the filter also
    /// reads.
    pub(crate) search: String,
}

impl Item {
    /// A row with no note and nothing extra to search.
    pub(crate) fn new(label: impl Into<String>, fixed: bool) -> Self {
        let label = label.into();
        Self {
            search: label.clone(),
            label,
            note: None,
            fixed,
        }
    }

    /// The same row with a word on its right.
    ///
    /// The note is searchable too: it is drawn on the row, and a filter that
    /// read only the label would refuse a word the person can see.
    pub(crate) fn noted(mut self, note: impl Into<String>) -> Self {
        let note: String = note.into();
        self.search.push(' ');
        self.search.push_str(&note);
        self.note = Some(note);
        self
    }

    /// The same row, also reachable by this text.
    pub(crate) fn searched_by(mut self, extra: impl Into<String>) -> Self {
        let extra: String = extra.into();
        self.search.push(' ');
        self.search.push_str(&extra);
        self
    }
}

/// One keystroke, freed of the library that reads it.
///
/// The state machine below decides behaviour and has no terminal behind it; the
/// mapping from a real key event to this lives in [`press_of`], which is the one
/// part that cannot be tested without a tty and is therefore kept trivial.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Press {
    /// Move the cursor up one row.
    Up,
    /// Move the cursor down one row.
    Down,
    /// Move the cursor up a screen.
    PageUp,
    /// Move the cursor down a screen.
    PageDown,
    /// Put the cursor on the first row.
    Home,
    /// Put the cursor on the last row.
    End,
    /// Take the row under the cursor.
    Enter,
    /// Leave without choosing.
    Escape,
    /// `Ctrl-C`: leave without choosing.
    Interrupt,
    /// `Ctrl-U`: clear the filter.
    Clear,
    /// Delete the last filter character.
    Backspace,
    /// Add a character to the filter.
    Char(char),
    /// A key the menu has no meaning for.
    Other,
}
/// What a key asked the menu to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    /// The menu goes on: something about it changed.
    Continue,
    /// The entry under the cursor was chosen.
    Accept,
    /// The person left without choosing.
    Cancel,
}
/// A list and where the cursor is on it.
#[derive(Clone, Debug)]
pub(crate) struct Menu {
    items: Vec<Item>,
    /// The entries the filter kept, as indices into `items`.
    visible: Vec<usize>,
    /// Where the cursor is *in `visible`*.
    cursor: usize,
    /// What was typed to narrow the list.
    filter: String,
}
impl Menu {
    /// A menu over every entry, with the cursor on the first.
    pub(crate) fn new(items: Vec<Item>) -> Self {
        let visible = (0..items.len()).collect();
        Self {
            items,
            visible,
            cursor: 0,
            filter: String::new(),
        }
    }

    /// The entries the filter keeps, as indices into the whole list.
    ///
    /// Case-insensitively, and over the label plus what the row was searched by:
    /// a person typing `55fd` means the session whose key starts with it, and
    /// one typing `footer` means the session that was named after one.
    fn matching(&self) -> Vec<usize> {
        if self.filter.is_empty() {
            return (0..self.items.len()).collect();
        }
        let needle = self.filter.to_ascii_lowercase();
        self.items
            .iter()
            .enumerate()
            .filter(|(_, item)| item.search.to_ascii_lowercase().contains(&needle))
            .map(|(index, _)| index)
            .collect()
    }

    /// Rebuilds the visible set after the filter changed, keeping the cursor in
    /// range: a filter that shrinks the list must not leave the cursor past its
    /// end, where Enter would choose nothing.
    fn refilter(&mut self) {
        self.visible = self.matching();
        self.cursor = self.cursor.min(self.visible.len().saturating_sub(1));
    }

    /// How many entries the filter kept.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.visible.len()
    }

    /// Whether the filter kept nothing.
    pub(crate) fn is_empty(&self) -> bool {
        self.visible.is_empty()
    }

    /// The entry under the cursor, as an index into the whole list.
    pub(crate) fn selected(&self) -> Option<usize> {
        self.visible.get(self.cursor).copied()
    }

    /// The entry under the cursor.
    #[cfg(test)]
    pub(crate) fn selected_item(&self) -> Option<&Item> {
        self.selected().and_then(|index| self.items.get(index))
    }

    /// What was typed to narrow the list.
    pub(crate) fn filter(&self) -> &str {
        &self.filter
    }

    /// Moves the cursor by a signed number of rows, holding at both ends.
    ///
    /// The list neither wraps nor stops silently: walking off the end holds at
    /// the end, which is what a file manager does and what keeps the first row
    /// reachable from anywhere by holding Up. Wrapping would make the row a
    /// person lands on depend on how many times they pressed a key.
    pub(crate) fn move_by(&mut self, delta: isize) {
        if self.visible.is_empty() {
            return;
        }
        let last = self.visible.len() - 1;
        self.cursor = if delta < 0 {
            self.cursor.saturating_sub(delta.unsigned_abs())
        } else {
            self.cursor.saturating_add(delta.unsigned_abs()).min(last)
        };
    }

    /// Puts the cursor on the first row.
    pub(crate) fn first(&mut self) {
        self.cursor = 0;
    }

    /// Puts the cursor on the last row.
    pub(crate) fn last(&mut self) {
        self.cursor = self.visible.len().saturating_sub(1);
    }

    /// Adds a character to the filter and re-runs it.
    pub(crate) fn push(&mut self, character: char) {
        self.filter.push(character);
        self.refilter();
    }

    /// Removes the last character of the filter.
    pub(crate) fn pop(&mut self) {
        self.filter.pop();
        self.refilter();
    }

    /// Clears the filter, which puts the whole list back.
    pub(crate) fn clear_filter(&mut self) {
        self.filter.clear();
        self.refilter();
    }

    /// The rows to draw: the visible slice, how many entries are hidden above
    /// it and how many below.
    ///
    /// The window follows the cursor rather than being fixed at the top: a list
    /// longer than the screen would otherwise hide the row the person is walking
    /// toward, which is the one row that has to be on screen.
    pub(crate) fn window(&self) -> (std::ops::Range<usize>, usize, usize) {
        if self.visible.len() <= VISIBLE_ROWS {
            return (0..self.visible.len(), 0, 0);
        }
        // The cursor sits a little below the middle of the window, so there is
        // context on both sides of it.
        let start = self
            .cursor
            .saturating_sub(VISIBLE_ROWS / 2)
            .min(self.visible.len() - VISIBLE_ROWS);
        let end = start + VISIBLE_ROWS;
        (start..end, start, self.visible.len() - end)
    }

    /// What one key means here.
    ///
    /// This is the whole keymap, as a method rather than a branch inside the
    /// reader, so the decided behaviour is what the tests drive.
    pub(crate) fn key(&mut self, press: Press) -> Step {
        match press {
            Press::Up => {
                self.move_by(-1);
                Step::Continue
            }
            Press::Down => {
                self.move_by(1);
                Step::Continue
            }
            Press::PageUp => {
                self.move_by(-page());
                Step::Continue
            }
            Press::PageDown => {
                self.move_by(page());
                Step::Continue
            }
            Press::Home => {
                self.first();
                Step::Continue
            }
            Press::End => {
                self.last();
                Step::Continue
            }
            Press::Enter => Step::Accept,
            // Escape is "not this one" and Ctrl-C is "not now". They are the same
            // decision to the menu — the caller is the one that decides what no
            // answer means — so they leave the same way.
            Press::Escape | Press::Interrupt => Step::Cancel,
            // Backspace on an empty filter has nothing left to delete, and
            // treating it as a cancel would leave the menu on the keystroke a
            // person pressed to correct a typo.
            Press::Backspace => {
                if !self.filter.is_empty() {
                    self.pop();
                }
                Step::Continue
            }
            Press::Clear => {
                self.clear_filter();
                Step::Continue
            }
            Press::Char(character) => {
                self.push(character);
                Step::Continue
            }
            Press::Other => Step::Continue,
        }
    }
}

/// How many rows a page key moves the cursor.
///
/// One screen, which is what makes `PageDown` land the list where it was drawn.
fn page() -> isize {
    isize::try_from(VISIBLE_ROWS).unwrap_or(isize::MAX)
}

/// Drops SGR sequences, so a whole row can be repainted in reverse video.
///
/// The selected row is drawn inverted, and nesting a colour inside the inversion
/// leaves the two fighting: the inner reset turns the highlight off halfway
/// across the line.
pub(crate) fn strip_style(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(character) = chars.next() {
        if character != '\x1b' {
            out.push(character);
            continue;
        }
        // An escape sequence ends at its final byte, and only `[` and `]`
        // introduce one that carries parameters. Eating everything up to the
        // next `m` — which is what this did — swallowed the text between a
        // cursor move and the colour that followed it, and left the colour's
        // own parameters behind: `[26;10H154;160;166m` on screen is that
        // mistake, and it is why a footer came out with escapes in it.
        match chars.next() {
            Some('[') => {
                // CSI: parameters, then a final byte in `@`..`~`.
                for next in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&next) {
                        break;
                    }
                }
            }
            Some(']') => {
                // OSC: runs to BEL or to the ST terminator.
                let mut previous = '\0';
                for next in chars.by_ref() {
                    if next == '\u{7}' || (previous == '\x1b' && next == '\\') {
                        break;
                    }
                    previous = next;
                }
            }
            // A two-character sequence, or a lone escape at the end.
            Some(_) | None => {}
        }
    }
    out
}

/// What running a menu produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// The entry under the cursor, as an index into the items.
    Chosen(usize),
    /// The filter as typed, matching no entry.
    ///
    /// A provider with models newer than the catalogue, a session key that fell
    /// off the end of a bounded list: both menus took a typed name before they
    /// had a cursor, and a filter that matched nothing is that same name
    /// reaching the same place. Dropping it would make the menus strictly
    /// smaller than the prompts they replaced.
    Typed(String),
    /// The person left without answering.
    Left,
}

/// The one dim line under the list that says what the filter is doing.
///
/// It is a function of the filter alone rather than part of the drawing, so
/// what it says is checked without a terminal to read it back from.
fn hint(filter: &str) -> String {
    if filter.is_empty() {
        "↑↓ move · enter open · esc back · type to filter".to_owned()
    } else {
        format!("↑↓ move · enter open · esc back · filter `{filter}` (ctrl-u clears)")
    }
}

/// Runs a menu until the person chooses or leaves.
///
/// The answer is the caller's to read: [`Outcome::Chosen`] indexes the `items`
/// that were passed in, and what nobody answered is [`Outcome::Left`], because
/// whether that is an error or a default is not the menu's decision to make.
///
/// A frame is drawn when something changed and not otherwise: the loop is
/// blocked on reading a key, so there is nothing to redraw between them, and
/// the terminal is one thing this process does not have to keep busy.
#[cfg(unix)]
pub(crate) fn run(items: Vec<Item>, title: &str, style: Style) -> Result<Outcome, String> {
    use ratatui::crossterm::event::{self, Event, KeyEventKind};

    if !crate::terminal::is_interactive() {
        return Err(format!(
            "`{title}` needs a terminal to be answered on; answer it on the command line instead"
        ));
    }
    let mut menu = Menu::new(items);
    let mut terminal = crate::terminal::Guard::enter()?;
    draw(&mut terminal, &menu, title, style)?;
    let outcome = loop {
        let Ok(event) = event::read() else {
            // A terminal that cannot be read is a terminal that cannot answer,
            // which leaves the way Escape does.
            break Outcome::Left;
        };
        let Event::Key(key) = event else {
            continue;
        };
        if key.kind == KeyEventKind::Release {
            continue;
        }
        match menu.key(press_of(key)) {
            Step::Continue => draw(&mut terminal, &menu, title, style)?,
            Step::Accept => break chosen(&menu),
            Step::Cancel => break Outcome::Left,
        }
    };
    Ok(outcome)
}

/// Draws one frame of the menu.
///
/// The layout is what decides where everything goes: the list takes what is
/// left after the title and the hint, and the rows inside it are cut to the
/// width the terminal has at that moment. Nothing here knows a number of
/// columns or rows, which is what makes a resize one more frame.
#[cfg(unix)]
fn draw(
    terminal: &mut crate::terminal::Guard,
    menu: &Menu,
    title: &str,
    style: Style,
) -> Result<(), String> {
    use ratatui::layout::{Constraint, Layout};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, List, ListItem, ListState, Paragraph};

    let (rows, above, below) = menu.window();
    let hidden = above + below;
    let mut state = ListState::default();
    state.select(Some(menu.cursor));

    // The rows are built here and drawn by the widget, so nothing in this
    // function writes to the terminal: the frame is one buffer, and the buffer
    // is what rustui diffs against the last one.
    let listed: Vec<ListItem> = rows
        .clone()
        .map(|index| {
            let item = &menu.items[menu.visible[index]];
            let mut line = Line::from(vec![Span::raw(item.label.clone())]);
            if let Some(note) = &item.note {
                line.push_span(Span::styled(format!(" · {note}"), style.dim_span()));
            }
            ListItem::new(line)
        })
        .collect();
    // What the row says depends on what is left to say: a filter that matched
    // nothing says so, a list longer than the window counts what it hid, and
    // the rest of the time it names the keys.
    let footer = if menu.is_empty() {
        format!("nothing matches `{}`", menu.filter())
    } else if hidden > 0 {
        format!("… {hidden} more, type to narrow the list")
    } else {
        hint(menu.filter())
    };

    terminal.draw(|frame| {
        let area = frame.area();
        // What the menu needs, in the order it reads: the title, the rows, the
        // hint. The rows take the slack and the others keep their one line.
        let [title_area, list_area, hint_area] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Fill(1),
            Constraint::Length(1),
        ])
        .areas(area);
        frame.render_widget(
            Paragraph::new(Line::styled(title.to_owned(), style.dim_span())),
            title_area,
        );
        let list = List::new(listed)
            .highlight_style(Style::selected())
            .highlight_symbol("› ");
        frame.render_stateful_widget(list, list_area, &mut state);
        frame.render_widget(
            Paragraph::new(Line::styled(footer, style.faint_span())).block(Block::new()),
            hint_area,
        );
    })
}

/// What Enter produced: a row, or the typed name when there was no row.
fn chosen(menu: &Menu) -> Outcome {
    menu.selected()
        .map_or_else(|| Outcome::Typed(menu.filter().to_owned()), Outcome::Chosen)
}

/// The menu without a terminal to walk it: the caller falls back to its prompt.
#[cfg(not(unix))]
pub(crate) fn run(_items: Vec<Item>, _title: &str, _style: Style) -> Result<Outcome, String> {
    Err("the arrow-key menu needs a Unix terminal".to_owned())
}

/// A key event as the menu's own keystroke.
#[cfg(unix)]
fn press_of(key: crossterm::event::KeyEvent) -> Press {
    use crossterm::event::{KeyCode, KeyModifiers};

    let control = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Up => Press::Up,
        KeyCode::Down => Press::Down,
        KeyCode::PageUp => Press::PageUp,
        KeyCode::PageDown => Press::PageDown,
        KeyCode::Home => Press::Home,
        KeyCode::End => Press::End,
        KeyCode::Enter => Press::Enter,
        KeyCode::Esc => Press::Escape,
        KeyCode::Char('c' | 'g') if control => Press::Interrupt,
        KeyCode::Char('u') if control => Press::Clear,
        KeyCode::Backspace => Press::Backspace,
        // A modified letter is not text: without this, `Ctrl-R` would filter for
        // an `r` the person never typed.
        KeyCode::Char(character) if !control && !key.modifiers.contains(KeyModifiers::ALT) => {
            Press::Char(character)
        }
        _ => Press::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn menu(labels: &[&str]) -> Menu {
        Menu::new(
            labels
                .iter()
                .map(|label| Item::new(*label, false))
                .collect(),
        )
    }

    fn rows() -> Vec<Item> {
        vec![
            Item::new("New session", true),
            Item::new("Last session", true).noted("unfinished"),
            Item::new("55fd07f3", false).searched_by("fix the footer"),
            Item::new("69be4643", false).searched_by("write the guide"),
        ]
    }

    #[test]
    fn the_cursor_walks_the_list_and_holds_at_both_ends() {
        let mut menu = menu(&["new", "last", "one", "two"]);
        assert_eq!(menu.selected(), Some(0));
        assert_eq!(menu.key(Press::Down), Step::Continue);
        assert_eq!(menu.selected(), Some(1));
        assert_eq!(menu.key(Press::Down), Step::Continue);
        assert_eq!(menu.selected(), Some(2));
        assert_eq!(menu.key(Press::Up), Step::Continue);
        assert_eq!(menu.selected(), Some(1));
        // The ends hold instead of wrapping: a wrap is how a person opens the
        // session at the other end of the list without ever seeing it.
        menu.key(Press::Home);
        menu.key(Press::Up);
        menu.key(Press::Up);
        assert_eq!(menu.selected(), Some(0), "up at the top stays at the top");
        menu.key(Press::End);
        assert_eq!(menu.selected(), Some(3));
        menu.key(Press::Down);
        assert_eq!(menu.selected(), Some(3), "down at the bottom stays");
    }

    #[test]
    fn enter_takes_the_row_under_the_cursor() {
        let mut menu = menu(&["new", "last", "one"]);
        menu.key(Press::Down);
        menu.key(Press::Down);
        assert_eq!(menu.key(Press::Enter), Step::Accept);
        assert_eq!(menu.selected(), Some(2));
        let item = menu.selected_item().expect("a row");
        assert_eq!(item.label, "one");
    }

    #[test]
    fn escape_and_control_c_leave_without_choosing() {
        let mut menu = menu(&["new", "last"]);
        assert_eq!(menu.key(Press::Escape), Step::Cancel);
        assert_eq!(menu.key(Press::Interrupt), Step::Cancel);
        // Leaving changes nothing: a caller that treats it as a default gets the
        // menu it already had.
        assert_eq!(menu.selected(), Some(0));
    }

    #[test]
    fn typing_narrows_the_list_and_backspace_widens_it_again() {
        let mut menu = Menu::new(rows());
        assert_eq!(menu.len(), 4);
        for character in "guide".chars() {
            assert_eq!(menu.key(Press::Char(character)), Step::Continue);
        }
        assert_eq!(menu.len(), 1, "only the guide session survives");
        assert_eq!(
            menu.selected_item().map(|item| item.label.as_str()),
            Some("69be4643")
        );
        menu.key(Press::Backspace);
        assert_eq!(menu.filter(), "guid");
        menu.key(Press::Clear);
        assert_eq!(menu.len(), 4, "clearing puts the whole list back");
    }

    #[test]
    fn the_filter_ignores_case_and_reads_what_the_row_carries() {
        let mut menu = Menu::new(rows());
        for character in "FOOTER".chars() {
            menu.push(character);
        }
        assert_eq!(menu.len(), 1, "the filter is case-insensitive");
        assert_eq!(
            menu.selected_item().map(|item| item.label.as_str()),
            Some("55fd07f3")
        );
        // The note is drawn on the row, so it filters too.
        menu.clear_filter();
        for character in "unfinished".chars() {
            menu.push(character);
        }
        assert_eq!(
            menu.selected_item().map(|item| item.label.as_str()),
            Some("Last session")
        );
        // And the key is in the label.
        menu.clear_filter();
        for character in "69be".chars() {
            menu.push(character);
        }
        assert_eq!(
            menu.selected_item().map(|item| item.label.as_str()),
            Some("69be4643")
        );
    }

    #[test]
    fn the_cursor_stays_on_a_row_when_the_filter_shrinks_the_list() {
        // The cursor is an index into the filtered list, so a filter that cuts
        // four rows down to one must not leave Enter choosing nothing.
        let mut menu = Menu::new(rows());
        menu.key(Press::End);
        assert_eq!(menu.selected(), Some(3));
        for character in "fix".chars() {
            menu.push(character);
        }
        assert_eq!(menu.len(), 1);
        assert_eq!(
            menu.selected(),
            Some(2),
            "the cursor moved onto the survivor"
        );
        // And a filter that keeps nothing leaves nothing to choose, which the
        // caller reports rather than acting on.
        menu.push('z');
        assert!(menu.is_empty());
        assert_eq!(menu.selected(), None);
        assert_eq!(menu.key(Press::Enter), Step::Accept);
        assert_eq!(menu.selected(), None);
    }

    #[test]
    fn backspace_on_an_empty_filter_does_not_leave_the_menu() {
        // It is the key a person presses to fix a typo. Turning it into a cancel
        // left the menu on the keystroke meant to keep it open.
        let mut menu = menu(&["new", "last"]);
        assert_eq!(menu.key(Press::Backspace), Step::Continue);
        assert_eq!(menu.filter(), "");
        assert_eq!(menu.len(), 2);
        assert_eq!(menu.key(Press::Enter), Step::Accept);
    }

    #[test]
    fn page_keys_and_the_ends_reach_the_rest_of_a_long_list() {
        let labels: Vec<String> = (0..40).map(|index| format!("session {index:02}")).collect();
        let borrowed: Vec<&str> = labels.iter().map(String::as_str).collect();
        let mut menu = menu(&borrowed);
        menu.key(Press::PageDown);
        assert_eq!(menu.selected(), Some(VISIBLE_ROWS));
        menu.key(Press::PageUp);
        assert_eq!(menu.selected(), Some(0));
        menu.key(Press::End);
        assert_eq!(menu.selected(), Some(39));
        menu.key(Press::PageDown);
        assert_eq!(menu.selected(), Some(39), "the end holds");
        menu.key(Press::Home);
        assert_eq!(menu.selected(), Some(0));
    }

    #[test]
    fn the_window_follows_the_cursor_and_counts_what_it_left_out() {
        let labels: Vec<String> = (0..40).map(|index| format!("model {index:02}")).collect();
        let borrowed: Vec<&str> = labels.iter().map(String::as_str).collect();
        let mut menu = menu(&borrowed);
        let (window, above, below) = menu.window();
        assert_eq!(window, 0..VISIBLE_ROWS, "a full first screen");
        assert_eq!(above, 0, "nothing above the cursor at the top");
        assert_eq!(below, 40 - VISIBLE_ROWS, "the rest is counted");
        menu.key(Press::End);
        let (window, above, below) = menu.window();
        assert_eq!(window.end, 40, "the last row is in view");
        assert_eq!(below, 0);
        assert_eq!(above, 40 - VISIBLE_ROWS);
        // A list that fits is drawn whole and hides nothing.
        let (window, above, below) =
            Menu::new(vec![Item::new("a", false), Item::new("b", false)]).window();
        assert_eq!(window.len(), 2);
        assert_eq!((above, below), (0, 0));
    }

    #[test]
    fn stripping_style_leaves_the_text_and_drops_the_codes() {
        let styled = format!("{}\x1b[0m {}", Style::plain().cyan("hex"), "value");
        assert_eq!(strip_style(&styled), "hex value");
        assert_eq!(strip_style("plain"), "plain");
    }

    #[test]
    fn enter_on_an_empty_filter_hands_back_what_was_typed() {
        // The menus were numbered prompts that took a typed name, and the name
        // of something the list does not hold is exactly what a filter that
        // matched nothing is. It is passed through rather than dropped.
        let mut menu = Menu::new(rows());
        for character in "river/deepseek".chars() {
            menu.push(character);
        }
        assert!(menu.is_empty());
        assert_eq!(chosen(&menu), Outcome::Typed("river/deepseek".to_owned()));
        // With a row under the cursor it is the row that wins.
        let mut menu = Menu::new(rows());
        menu.key(Press::Down);
        assert_eq!(chosen(&menu), Outcome::Chosen(1));
    }

    #[test]
    fn the_hint_says_what_the_filter_is() {
        assert!(hint("").contains("type to filter"), "{}", hint(""));
        assert!(hint("riv").contains("filter `riv`"), "{}", hint("riv"));
    }
}
