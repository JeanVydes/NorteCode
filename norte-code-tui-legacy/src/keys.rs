//! The keys the interface answers to.
//!
//! Every key lives here so the bindings have one owner. A view may bind keys of
//! its own, but a shortcut that acts on the whole interface is declared once in
//! this module and documented in the help modal.

use crossterm::event::KeyCode;

/// Quits the interface.
pub const QUIT_CHAR: char = 'q';
pub const QUIT: KeyCode = KeyCode::Char(QUIT_CHAR);

/// Opens the help modal.
pub const HELP_CHAR: char = '?';
pub const HELP: KeyCode = KeyCode::Char(HELP_CHAR);

/// Opens the command menu, and closes whatever overlay is open.
pub const MENU: KeyCode = KeyCode::Esc;
pub const CANCEL: KeyCode = KeyCode::Esc;

/// Moves the selection down.
pub const NAV_DOWN_CHAR: char = 'j';
pub const NAV_DOWN: KeyCode = KeyCode::Char(NAV_DOWN_CHAR);

/// Moves the selection up.
pub const NAV_UP_CHAR: char = 'k';
pub const NAV_UP: KeyCode = KeyCode::Char(NAV_UP_CHAR);

pub const NAV_DOWN_ARROW: KeyCode = KeyCode::Down;
pub const NAV_UP_ARROW: KeyCode = KeyCode::Up;
pub const NAV_LEFT: KeyCode = KeyCode::Left;
pub const NAV_RIGHT: KeyCode = KeyCode::Right;

/// Switches to the next tab, and to the previous one.
pub const NEXT_TAB: KeyCode = KeyCode::Tab;
pub const PREV_TAB: KeyCode = KeyCode::BackTab;

/// Confirms the selected row.
pub const SUBMIT: KeyCode = KeyCode::Enter;

/// Erases the character before the caret.
pub const ERASE: KeyCode = KeyCode::Backspace;

/// Jumps to the top of a scrolling view.
pub const TOP_CHAR: char = 'g';
pub const TOP: KeyCode = KeyCode::Char(TOP_CHAR);

/// Jumps to the bottom of a scrolling view.
pub const BOTTOM_CHAR: char = 'G';
pub const BOTTOM: KeyCode = KeyCode::Char(BOTTOM_CHAR);

/// Stops the selected background job.
pub const STOP_JOB_CHAR: char = 'S';

/// Focuses the message composer.
pub const COMPOSER_CHAR: char = 'i';
pub const COMPOSER: KeyCode = KeyCode::Char(COMPOSER_CHAR);

/// Starts a filter inside a view.
pub const SEARCH_CHAR: char = '/';

/// The label of the chord that stops the running turn.
pub const CANCEL_TURN_LABEL: &str = "Ctrl+C";

/// Whether a key is the chord that stops the running turn.
#[must_use]
pub fn is_cancel_turn(key: crossterm::event::KeyEvent) -> bool {
    key.code == KeyCode::Char('c')
        && key
            .modifiers
            .contains(crossterm::event::KeyModifiers::CONTROL)
}

/// Whether a key is the chord that starts an attachment.
#[must_use]
pub fn is_attach(key: crossterm::event::KeyEvent) -> bool {
    key.code == KeyCode::Char('a')
        && key
            .modifiers
            .contains(crossterm::event::KeyModifiers::CONTROL)
}

/// Whether a key asks for a new line instead of sending.
///
/// Terminals that cannot report Shift+Enter send Ctrl+J for it, so both chords
/// mean the same thing inside the composer.
#[must_use]
pub fn is_newline(key: crossterm::event::KeyEvent) -> bool {
    key.code == KeyCode::Char('j')
        && key
            .modifiers
            .contains(crossterm::event::KeyModifiers::CONTROL)
}
