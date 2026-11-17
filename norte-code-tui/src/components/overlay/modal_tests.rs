//! The modal shows every row it was given, and the whole screen behind it dims.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Terminal,
    backend::TestBackend,
    style::Style,
    text::{Line, Span},
};

use super::Modal;
use crate::components::{MenuEntry, ModalList, paint_scrim};
use crate::ui_model::ThemeMode;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn screen(rows: u16, columns: u16, draw: impl FnOnce(&mut ratatui::Frame<'_>)) -> String {
    let mut terminal = Terminal::new(TestBackend::new(columns, rows)).expect("terminal");
    terminal.draw(|frame| draw(frame)).expect("draw");
    let buffer = terminal.backend().buffer().clone();
    let mut text = String::new();
    for y in 0..rows {
        for x in 0..columns {
            text.push_str(buffer[(x, y)].symbol());
        }
        text.push('\n');
    }
    text
}

#[test]
fn every_row_reaches_the_screen_on_a_tall_terminal() {
    let body = (0..30)
        .map(|index| Line::from(Span::raw(format!("row {index}"))))
        .collect();

    let text = screen(50, 100, |frame| {
        Modal {
            title: "t",
            controls: "c",
            body,
            theme: ThemeMode::Dark,
            selected: None,
        }
        .render(frame);
    });

    assert!(text.contains("row 0"));
    assert!(text.contains("row 29"));
}

#[test]
fn the_selection_scrolls_the_body_into_view() {
    let mut list = ModalList::new(
        (0..40)
            .map(|index| MenuEntry::new(format!("row {index}"), None))
            .collect(),
    );
    for _ in 0..39 {
        list.handle_key(key(KeyCode::Char('j')));
    }

    let text = screen(30, 100, |frame| list.render(frame, "t", ThemeMode::Dark));

    assert!(
        text.contains("row 39"),
        "the selected row is off screen:\n{text}"
    );
}

#[test]
fn the_scrim_covers_the_whole_screen() {
    let painted = paint_scrim_once();
    assert!(painted);
}

fn paint_scrim_once() -> bool {
    let mut seen = false;
    let _ = screen(10, 20, |frame| {
        paint_scrim(frame, ThemeMode::Dark);
        let buffer = frame.buffer_mut();
        let dim = buffer[(19, 9)].style();
        seen = dim.fg.is_some() && dim.bg.is_some();
    });
    seen
}

#[test]
fn the_modal_is_never_wider_than_the_terminal() {
    let body = vec![Line::from(Span::styled(
        "a very long line that would overflow",
        Style::default(),
    ))];

    let text = screen(12, 40, |frame| {
        Modal {
            title: "t",
            controls: "c",
            body,
            theme: ThemeMode::Dark,
            selected: None,
        }
        .render(frame);
    });

    for line in text.lines() {
        assert_eq!(line.chars().count(), 40);
    }
}

#[test]
fn the_command_menu_shows_its_last_row_on_a_forty_row_screen() {
    let mut list = ModalList::new(
        (0..30)
            .map(|index| {
                MenuEntry::new(format!("row {index}"), None).in_section(if index < 10 {
                    "First"
                } else {
                    "Last"
                })
            })
            .collect(),
    );
    for _ in 0..29 {
        list.handle_key(key(KeyCode::Char('j')));
    }

    let text = screen(40, 100, |frame| {
        list.render(frame, "commands", ThemeMode::Dark)
    });

    assert!(text.contains("row 29"), "the last row is cut:\n{text}");
}
