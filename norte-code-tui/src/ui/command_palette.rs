//! The command palette: a typed command surface over the existing shortcuts.

use std::any::Any;
use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use serde::Serialize;
use terminalui::components::commandpalette::{Command, CommandPalette as Palette};
use terminalui::{Context, InputEvent, Node, Outcome, Theme, View};

use super::copy;
use crate::ui_model::Tab;

/// What the host should do after a command is picked.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub(crate) enum PaletteAction {
    /// Show a tab.
    SwitchTab(Tab),
    /// Move to the next theme.
    CycleTheme,
    /// Refetch sessions and catalogue data.
    Refresh,
    /// Start a new session.
    NewSession,
    /// Compact the attached session's context.
    Compact,
}

/// The palette plus the focus to restore when it closes.
#[derive(Serialize)]
pub(crate) struct CommandPalette {
    palette: Palette,
    #[serde(skip)]
    return_focus: Option<String>,
}

impl CommandPalette {
    pub(crate) fn new() -> Self {
        let commands = Tab::ALL
            .iter()
            .map(|tab| {
                Command::new(copy::switch_tab_command(tab.label())).detail(copy::COMMAND_GROUP_TAB)
            })
            .chain([
                Command::new(copy::COMMAND_CYCLE_THEME).detail(copy::COMMAND_GROUP_VIEW),
                Command::new(copy::COMMAND_REFRESH).detail(copy::COMMAND_GROUP_SESSION),
                Command::new(copy::COMMAND_NEW_SESSION).detail(copy::COMMAND_GROUP_SESSION),
                Command::new(copy::COMMAND_COMPACT).detail(copy::COMMAND_GROUP_SESSION),
            ])
            .collect();
        Self {
            palette: Palette::new(commands),
            return_focus: None,
        }
    }

    /// Open the palette and remember where focus came from.
    pub(crate) fn open(&mut self, return_focus: Option<&str>) {
        self.return_focus = return_focus.map(str::to_owned);
        self.palette.open();
    }

    /// Toggle the palette. Closing it returns the focus to restore.
    pub(crate) fn toggle(&mut self, return_focus: Option<&str>) -> Option<String> {
        if self.is_open() {
            self.close();
            self.return_focus.take()
        } else {
            self.open(return_focus);
            None
        }
    }

    pub(crate) fn close(&mut self) {
        self.palette.close();
        self.return_focus = None;
    }

    pub(crate) fn is_open(&self) -> bool {
        self.palette.is_open()
    }

    /// Take the activation and the focus to restore, if the palette just closed.
    pub(crate) fn take_action(&mut self) -> Option<(PaletteAction, Option<String>)> {
        let index = self.palette.take_activated()?;
        let action = match index {
            0 => PaletteAction::SwitchTab(Tab::Chat),
            1 => PaletteAction::SwitchTab(Tab::Sessions),
            2 => PaletteAction::SwitchTab(Tab::Models),
            3 => PaletteAction::SwitchTab(Tab::Telemetry),
            4 => PaletteAction::SwitchTab(Tab::Settings),
            5 => PaletteAction::CycleTheme,
            6 => PaletteAction::Refresh,
            7 => PaletteAction::NewSession,
            8 => PaletteAction::Compact,
            _ => return None,
        };
        Some((action, self.return_focus.take()))
    }

    /// Take the focus to restore after an Esc or outside click closed the palette.
    pub(crate) fn take_return_focus(&mut self) -> Option<String> {
        if self.is_open() {
            return None;
        }
        self.return_focus.take()
    }
}

impl View for CommandPalette {
    fn as_any(&mut self) -> &mut dyn Any {
        self
    }

    fn as_any_ref(&self) -> &dyn Any {
        self
    }

    fn kind(&self) -> &'static str {
        "command_palette"
    }

    fn focusable(&self) -> bool {
        self.palette.focusable()
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut Context, theme: &Theme) {
        self.palette.render(area, buf, cx, theme);
    }

    fn handle_event(&mut self, event: &InputEvent, cx: &mut Context) -> Outcome {
        self.palette.handle_event(event, cx)
    }

    fn tick(&mut self, delta: Duration, cx: &mut Context) {
        self.palette.tick(delta, cx);
    }

    fn children(&self) -> &[Node] {
        self.palette.children()
    }

    fn children_mut(&mut self) -> &mut [Node] {
        self.palette.children_mut()
    }
}
