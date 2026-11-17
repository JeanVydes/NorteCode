//! The command menu: the one place the reader finds a command.
//!
//! It replaces the scattered per-view shortcuts. The rows are built from a
//! snapshot of the model taken when the menu opens, so a choice shows its
//! current value.

use crossterm::event::KeyEvent;

use crate::components::overlay::list::ListOutcome;
use crate::components::{CommandMenuAction, MenuEntry, ModalList, Preference};
use crate::keys;
use crate::tools::DiffMode;
use crate::ui_model::{ReasoningLevel, SendMode, ThemeMode, ThinkingVisibility, UiModel};

const SECTION_SESSION: &str = "Session";
const SECTION_MESSAGE: &str = "Message";
const SECTION_VIEW: &str = "View";
const SECTION_HELP: &str = "Help";

/// What the menu reads from the model, frozen when it opens.
#[allow(clippy::struct_excessive_bools)]
struct Snapshot {
    send_mode: SendMode,
    reasoning: ReasoningLevel,
    budget_action: norte_code_wire::BudgetAction,
    diff: DiffMode,
    theme: ThemeMode,
    thinking: ThinkingVisibility,
    timestamps: bool,
    tool_output: bool,
    shell_output_expanded: bool,
    live_refresh: bool,
    developer_mode: bool,
}

impl Snapshot {
    fn of(model: &UiModel) -> Self {
        Self {
            send_mode: model.send_mode,
            reasoning: model.reasoning,
            budget_action: model.budget_action,
            diff: model.cli.diff_style,
            theme: model.preferences.theme,
            thinking: model.preferences.thinking,
            timestamps: model.preferences.show_timestamps.is_on(),
            tool_output: model.preferences.show_tool_output.is_on(),
            shell_output_expanded: model.shell_output_expanded.is_on(),
            live_refresh: model.preferences.auto_refresh_sessions.is_on(),
            developer_mode: model.preferences.developer_mode.is_on(),
        }
    }
}

/// A row the menu offers, with the command it runs.
struct Row {
    entry: MenuEntry,
    action: CommandMenuAction,
}

/// The command menu, with its rows and the choice opened over them.
#[derive(Default)]
pub(crate) struct CommandMenu {
    snapshot: Option<Snapshot>,
    rows: Vec<Row>,
    list: Option<ModalList>,
    choice: Option<Choice>,
}

/// The outcomes of a key pressed in the menu.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CommandOutcome {
    /// The menu consumed the key.
    Handled,
    /// The reader picked a command.
    Run(CommandMenuAction),
    /// The menu is not open and the key is not its own.
    Ignored,
}

impl CommandMenu {
    /// Whether the menu or a choice over it is open.
    pub(crate) fn is_open(&self) -> bool {
        self.list.is_some() || self.choice.is_some()
    }

    /// Opens the menu with rows built from the model.
    pub(crate) fn open(&mut self, model: &UiModel) {
        let snapshot = Snapshot::of(model);
        self.rows = rows_for(&snapshot);
        self.list = Some(ModalList::new(
            self.rows.iter().map(|row| row.entry.clone()).collect(),
        ));
        self.snapshot = Some(snapshot);
        self.choice = None;
    }

    /// Closes the menu and anything opened over it.
    pub(crate) fn close(&mut self) {
        self.list = None;
        self.choice = None;
        self.snapshot = None;
        self.rows.clear();
    }

    /// Handles a key.
    ///
    /// `Esc` closes the choice first, then the menu, then drops a filter.
    pub(crate) fn handle_key(&mut self, key: KeyEvent) -> CommandOutcome {
        if self.choice.is_some() {
            return self.handle_choice_key(key);
        }
        let Some(list) = self.list.as_mut() else {
            return CommandOutcome::Ignored;
        };
        match list.handle_key(key) {
            ListOutcome::Closed => {
                if !list.clear_filter() {
                    self.close();
                }
                CommandOutcome::Handled
            }
            ListOutcome::Submitted(index) => self.submit(index),
            ListOutcome::Handled | ListOutcome::Ignored => CommandOutcome::Handled,
        }
    }

    fn submit(&mut self, index: usize) -> CommandOutcome {
        let Some(row) = self.rows.get(index) else {
            return CommandOutcome::Handled;
        };
        match row.action {
            CommandMenuAction::SetQueueMode(_) => {
                self.open_choice(ChoiceKind::Queue);
                CommandOutcome::Handled
            }
            CommandMenuAction::SetReasoning(_) => {
                self.open_choice(ChoiceKind::Reasoning);
                CommandOutcome::Handled
            }
            CommandMenuAction::SetBudgetAction(_) => {
                self.open_choice(ChoiceKind::Budget);
                CommandOutcome::Handled
            }
            action => {
                self.close();
                CommandOutcome::Run(action)
            }
        }
    }

    fn handle_choice_key(&mut self, key: KeyEvent) -> CommandOutcome {
        let Some(choice) = self.choice.as_mut() else {
            return CommandOutcome::Ignored;
        };
        match choice.list.handle_key(key) {
            ListOutcome::Closed => {
                if choice.list.clear_filter() {
                    CommandOutcome::Handled
                } else {
                    self.choice = None;
                    CommandOutcome::Handled
                }
            }
            ListOutcome::Submitted(index) => {
                let action = choice.values.get(index).map(|value| value.action);
                match action {
                    Some(action) => {
                        self.close();
                        CommandOutcome::Run(action)
                    }
                    None => CommandOutcome::Handled,
                }
            }
            ListOutcome::Handled | ListOutcome::Ignored => CommandOutcome::Handled,
        }
    }

    fn open_choice(&mut self, kind: ChoiceKind) {
        let Some(snapshot) = self.snapshot.as_ref() else {
            return;
        };
        let (title, values) = kind.build(snapshot);
        let entries = values.iter().map(|value| value.entry.clone()).collect();
        self.choice = Some(Choice {
            title,
            list: ModalList::new(entries),
            values,
        });
    }

    /// Draws the open choice, or the open menu.
    pub(crate) fn render(&self, frame: &mut ratatui::Frame<'_>, theme: ThemeMode) {
        if let Some(choice) = &self.choice {
            choice.list.render(frame, choice.title, theme);
        } else if let Some(list) = &self.list {
            list.render(frame, "commands", theme);
        }
    }
}

/// A row a choice offers, with what picking it runs.
#[derive(Clone)]
pub(crate) struct ChoiceValue {
    pub(crate) entry: MenuEntry,
    pub(crate) action: CommandMenuAction,
}

struct Choice {
    title: &'static str,
    list: ModalList,
    values: Vec<ChoiceValue>,
}

#[derive(Clone, Copy)]
enum ChoiceKind {
    Queue,
    Reasoning,
    Budget,
}

impl ChoiceKind {
    fn build(self, snapshot: &Snapshot) -> (&'static str, Vec<ChoiceValue>) {
        match self {
            Self::Queue => ("queue mode", queue_values(snapshot.send_mode)),
            Self::Reasoning => ("reasoning effort", reasoning_values(snapshot.reasoning)),
            Self::Budget => ("budget action", budget_values(snapshot.budget_action)),
        }
    }
}

pub(crate) fn queue_values(current: SendMode) -> Vec<ChoiceValue> {
    [
        SendMode::Steer,
        SendMode::Queue,
        SendMode::Interrupt,
        SendMode::Reject,
    ]
    .into_iter()
    .map(|mode| ChoiceValue {
        entry: MenuEntry::new(mode.label(), Some(mode.description().to_owned()))
            .current(mode == current),
        action: CommandMenuAction::SetQueueMode(mode),
    })
    .collect()
}

pub(crate) fn reasoning_values(current: ReasoningLevel) -> Vec<ChoiceValue> {
    [
        ReasoningLevel::Automatic,
        ReasoningLevel::Disabled,
        ReasoningLevel::Low,
        ReasoningLevel::Medium,
        ReasoningLevel::High,
    ]
    .into_iter()
    .map(|level| ChoiceValue {
        entry: MenuEntry::new(level.label(), None).current(level == current),
        action: CommandMenuAction::SetReasoning(level),
    })
    .collect()
}

pub(crate) fn budget_values(current: norte_code_wire::BudgetAction) -> Vec<ChoiceValue> {
    [
        norte_code_wire::BudgetAction::Warn,
        norte_code_wire::BudgetAction::Ask,
        norte_code_wire::BudgetAction::Stop,
    ]
    .into_iter()
    .map(|action| ChoiceValue {
        entry: MenuEntry::new(budget_action_label(action), None).current(action == current),
        action: CommandMenuAction::SetBudgetAction(action),
    })
    .collect()
}

#[allow(clippy::too_many_lines)]
fn rows_for(snapshot: &Snapshot) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut push = |label: &str, hint: Option<String>, section: &str, action| {
        rows.push(Row {
            entry: MenuEntry::new(label, hint).in_section(section),
            action,
        });
    };

    push(
        "New session",
        Some("n".to_owned()),
        SECTION_SESSION,
        CommandMenuAction::NewSession,
    );
    push(
        "Export session",
        None,
        SECTION_SESSION,
        CommandMenuAction::ExportSession,
    );
    push(
        "Import session",
        None,
        SECTION_SESSION,
        CommandMenuAction::ImportSession,
    );
    push(
        "Compact session",
        None,
        SECTION_SESSION,
        CommandMenuAction::CompactSession,
    );
    push(
        "Rename session",
        None,
        SECTION_SESSION,
        CommandMenuAction::RenameSession,
    );
    push(
        "Fork session",
        None,
        SECTION_SESSION,
        CommandMenuAction::ForkSession,
    );
    push(
        "Delete session",
        None,
        SECTION_SESSION,
        CommandMenuAction::DeleteSession,
    );

    push(
        "Queue mode",
        Some(snapshot.send_mode.label().to_owned()),
        SECTION_MESSAGE,
        CommandMenuAction::SetQueueMode(snapshot.send_mode),
    );
    push(
        "Reasoning effort",
        Some(snapshot.reasoning.label().to_owned()),
        SECTION_MESSAGE,
        CommandMenuAction::SetReasoning(snapshot.reasoning),
    );
    push(
        "Rewind to a turn",
        None,
        SECTION_MESSAGE,
        CommandMenuAction::RewindToTurn,
    );
    push(
        "Pin the current turn",
        None,
        SECTION_MESSAGE,
        CommandMenuAction::PinCurrentTurn,
    );
    push(
        "Budget limit",
        None,
        SECTION_MESSAGE,
        CommandMenuAction::SetBudgetLimit,
    );
    push(
        "Budget action",
        Some(budget_action_label(snapshot.budget_action).to_owned()),
        SECTION_MESSAGE,
        CommandMenuAction::SetBudgetAction(snapshot.budget_action),
    );

    push(
        "Refresh current view",
        None,
        SECTION_VIEW,
        CommandMenuAction::RefreshView,
    );
    push(
        "Diff mode",
        Some(diff_label(snapshot.diff).to_owned()),
        SECTION_VIEW,
        CommandMenuAction::SetDiffStyle(snapshot.diff),
    );
    push(
        "Shell output",
        Some(
            if snapshot.shell_output_expanded {
                "expanded"
            } else {
                "collapsed"
            }
            .to_owned(),
        ),
        SECTION_VIEW,
        CommandMenuAction::ToggleShellOutput,
    );
    push(
        "Theme",
        Some(theme_label(snapshot.theme).to_owned()),
        SECTION_VIEW,
        CommandMenuAction::TogglePreference(Preference::Theme),
    );
    push(
        "Thinking",
        Some(thinking_label(snapshot.thinking).to_owned()),
        SECTION_VIEW,
        CommandMenuAction::TogglePreference(Preference::Thinking),
    );
    push(
        "Timestamps",
        Some(on_off(snapshot.timestamps).to_owned()),
        SECTION_VIEW,
        CommandMenuAction::TogglePreference(Preference::Timestamps),
    );
    push(
        "Tool output",
        Some(on_off(snapshot.tool_output).to_owned()),
        SECTION_VIEW,
        CommandMenuAction::TogglePreference(Preference::ToolOutput),
    );
    push(
        "Live session refresh",
        Some(on_off(snapshot.live_refresh).to_owned()),
        SECTION_VIEW,
        CommandMenuAction::TogglePreference(Preference::LiveRefresh),
    );
    push(
        "Developer diagnostics",
        Some(on_off(snapshot.developer_mode).to_owned()),
        SECTION_VIEW,
        CommandMenuAction::TogglePreference(Preference::DeveloperMode),
    );

    push(
        "Help",
        Some(keys::HELP_CHAR.to_string()),
        SECTION_HELP,
        CommandMenuAction::Help,
    );
    push(
        "Quit",
        Some(keys::QUIT_CHAR.to_string()),
        SECTION_HELP,
        CommandMenuAction::Quit,
    );
    rows
}

pub(crate) const fn budget_action_label(action: norte_code_wire::BudgetAction) -> &'static str {
    match action {
        norte_code_wire::BudgetAction::Warn => "warn, continue",
        norte_code_wire::BudgetAction::Ask => "ask before continuing",
        norte_code_wire::BudgetAction::Stop => "stop the turn",
    }
}

const fn diff_label(mode: DiffMode) -> &'static str {
    match mode {
        DiffMode::Unified => "unified",
        DiffMode::Split => "split",
    }
}

const fn theme_label(theme: ThemeMode) -> &'static str {
    match theme {
        ThemeMode::Dark => "dark",
        ThemeMode::Light => "light",
    }
}

const fn thinking_label(visibility: ThinkingVisibility) -> &'static str {
    match visibility {
        ThinkingVisibility::Collapsed => "collapsed",
        ThinkingVisibility::Expanded => "expanded",
    }
}

const fn on_off(enabled: bool) -> &'static str {
    if enabled { "on" } else { "off" }
}
