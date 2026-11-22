//! The plugins page: what is installed, and what can be done with it.
//!
//! Every action is on screen. The page used to hide install, update and remove
//! behind letters no one could see, with the shortcuts footer gone; a reader
//! who does not already know a key cannot discover it. The toolbar names each
//! action, and Enter on a row toggles the plugin.

use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

use crate::{
    components::{
        Component, ComponentAction, ComponentId, ComponentLifecycle, PluginOperation, ViewAction,
        palette, panel, spacing,
    },
    keys,
    text_builder::TextBuilder,
    ui_model::UiModel,
};

/// The actions the toolbar offers, in the order they are drawn.
const ACTIONS: [Action; 4] = [
    Action::Install,
    Action::Update,
    Action::UpdateAll,
    Action::Remove,
];

/// What the page can do, as a clickable button.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Action {
    Install,
    Update,
    UpdateAll,
    Remove,
}

impl Action {
    const fn label(self) -> &'static str {
        match self {
            Self::Install => "install",
            Self::Update => "update",
            Self::UpdateAll => "update all",
            Self::Remove => "remove",
        }
    }

    const fn hint(self) -> &'static str {
        match self {
            Self::Install => "a git repository the host clones and builds",
            Self::Update => "re-clone the selected plugin's source",
            Self::UpdateAll => "re-clone every plugin with a recorded source",
            Self::Remove => "delete the selected plugin's library",
        }
    }
}

/// The popup the page is showing over the list.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Mode {
    #[default]
    Listing,
    Repository,
    Package,
    ConfirmRemove,
}

#[derive(Default)]
pub(in crate::components) struct PluginsView {
    lifecycle: ComponentLifecycle,
    selected: usize,
    mode: Mode,
    repository: String,
    package: String,
    /// The action the pointer is over, so it reads as a button.
    hovered: Option<usize>,
    pending_action: Option<ComponentAction>,
    /// Where the toolbar was drawn, so a click maps to a button.
    toolbar: Rect,
}

impl Component for PluginsView {
    fn id(&self) -> ComponentId {
        ComponentId::Plugins
    }

    fn lifecycle(&self) -> &ComponentLifecycle {
        &self.lifecycle
    }

    fn lifecycle_mut(&mut self) -> &mut ComponentLifecycle {
        &mut self.lifecycle
    }

    fn captures_global_keys(&self) -> bool {
        self.mode != Mode::Listing
    }

    fn handle_mouse(&mut self, mouse: MouseEvent, model: &UiModel) {
        let position = ratatui::layout::Position {
            x: mouse.column,
            y: mouse.row,
        };
        match mouse.kind {
            MouseEventKind::Moved => {
                self.hovered = action_at(self.toolbar, mouse.column, mouse.row);
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(index) = action_at(self.toolbar, mouse.column, mouse.row) {
                    self.run(ACTIONS[index], model);
                }
            }
            _ => {}
        }
        let _ = position;
    }

    fn handle_key(&mut self, key: KeyEvent, model: &UiModel) {
        match self.mode {
            Mode::Repository | Mode::Package => {
                self.type_path(key.code);
                return;
            }
            Mode::ConfirmRemove => {
                match key.code {
                    keys::CANCEL => self.mode = Mode::Listing,
                    keys::SUBMIT => {
                        if let Some(plugin) = model.plugins.get(self.selected) {
                            self.pending_action = Some(ComponentAction::View(ViewAction::Plugin(
                                PluginOperation::Remove {
                                    name: plugin.name.clone(),
                                },
                            )));
                        }
                        self.mode = Mode::Listing;
                    }
                    _ => {}
                }
                return;
            }
            Mode::Listing => {}
        }
        match key.code {
            keys::NAV_DOWN | keys::NAV_DOWN_ARROW => {
                panel::move_selection(&mut self.selected, model.plugins.len(), true);
            }
            keys::NAV_UP | keys::NAV_UP_ARROW => {
                panel::move_selection(&mut self.selected, model.plugins.len(), false);
            }
            keys::SUBMIT => self.toggle_selected(model),
            KeyCode::Char('r') => {
                self.pending_action = Some(ComponentAction::View(ViewAction::RefreshPlugins));
            }
            _ => {}
        }
    }

    fn render(&mut self, frame: &mut Frame<'_>, area: Rect, model: &UiModel) {
        let progress = u16::from(model.plugin_job.is_some());
        let regions = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(progress),
                Constraint::Min(3),
            ])
            .split(area);
        self.toolbar = regions[0];
        self.render_toolbar(frame, regions[0], model);
        if let Some(job) = &model.plugin_job {
            frame.render_widget(
                Paragraph::new(progress_line(job, model, regions[1].width))
                    .style(palette::panel_style(model.preferences.theme)),
                regions[2],
            );
        }

        match self.mode {
            Mode::Repository => {
                Self::render_prompt(
                    frame,
                    regions[2],
                    model,
                    "install plugin",
                    "Repository URL",
                    &self.repository,
                    "Enter continues, Esc cancels",
                );
                return;
            }
            Mode::Package => {
                Self::render_prompt(
                    frame,
                    regions[2],
                    model,
                    "install plugin",
                    "Workspace package: optional, leave blank to install the whole workspace",
                    &self.package,
                    "Enter installs, Esc cancels",
                );
                return;
            }
            Mode::ConfirmRemove => {
                let name = model
                    .plugins
                    .get(self.selected)
                    .map_or("", |plugin| plugin.name.as_str());
                Self::render_prompt(
                    frame,
                    regions[2],
                    model,
                    "remove plugin",
                    &format!("Remove {name}?"),
                    "",
                    "Enter removes, Esc cancels",
                );
                return;
            }
            Mode::Listing => {}
        }

        let entries: Vec<Line<'static>> =
            model.plugins.iter().map(TextBuilder::plugin_row).collect();
        panel::render_list(
            frame,
            regions[2],
            entries.into_iter().map(|line| vec![line]).collect(),
            model.preferences.theme,
            panel::ListPanel {
                title: ComponentId::Plugins,
                phase: self.lifecycle.phase(),
                selected: Some(self.selected),
                focused: true,
                empty_message: TextBuilder::empty_message(ComponentId::Plugins),
            },
        );
    }

    fn take_action(&mut self) -> Option<ComponentAction> {
        self.pending_action.take()
    }
}

impl PluginsView {
    /// The toolbar: one button per action, and what the selected one does.
    fn render_toolbar(&self, frame: &mut Frame<'_>, area: Rect, model: &UiModel) {
        let theme = model.preferences.theme;
        let mut spans = vec![Span::raw(" ")];
        for (index, action) in ACTIONS.iter().enumerate() {
            let hovered = self.hovered == Some(index);
            spans.push(Span::styled(
                format!(" {} ", action.label()),
                Style::default()
                    .fg(if hovered {
                        palette::background(theme)
                    } else {
                        palette::accent(theme)
                    })
                    .bg(if hovered {
                        palette::accent(theme)
                    } else {
                        palette::surface_alt(theme)
                    })
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::raw(" "));
        }
        let hint = self
            .hovered
            .and_then(|index| ACTIONS.get(index))
            .map_or("Enter toggles the selected plugin", |action| action.hint());
        spans.push(Span::styled(
            format!("  {hint}"),
            palette::muted_style(theme),
        ));
        frame.render_widget(
            Paragraph::new(Line::from(spans)).style(palette::panel_style(theme)),
            area,
        );
    }

    /// A prompt box for the action that needs one.
    #[allow(clippy::too_many_arguments)]
    fn render_prompt(
        frame: &mut Frame<'_>,
        area: Rect,
        model: &UiModel,
        title: &str,
        caption: &str,
        value: &str,
        controls: &str,
    ) {
        let theme = model.preferences.theme;
        let body = vec![
            Line::from(Span::styled(
                caption.to_owned(),
                palette::muted_style(theme),
            )),
            Line::default(),
            Line::from(Span::styled(
                format!("{value}_"),
                palette::text_style(theme),
            )),
            Line::default(),
            Line::from(Span::styled(
                controls.to_owned(),
                palette::muted_style(theme),
            )),
        ];
        frame.render_widget(
            Paragraph::new(ratatui::text::Text::from(body))
                .style(palette::modal_style(theme))
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(palette::border_style(theme))
                        .title(format!(" {title} "))
                        .padding(spacing::panel())
                        .style(palette::modal_style(theme)),
                ),
            area,
        );
    }

    /// Starts what a toolbar button asks for.
    fn run(&mut self, action: Action, model: &UiModel) {
        match action {
            Action::Install => {
                self.repository.clear();
                self.package.clear();
                self.mode = Mode::Repository;
            }
            Action::Update => self.update_selected(model),
            Action::UpdateAll => {
                self.pending_action = Some(ComponentAction::View(ViewAction::Plugin(
                    PluginOperation::Update { name: None },
                )));
            }
            Action::Remove => {
                if model.plugins.get(self.selected).is_some() {
                    self.mode = Mode::ConfirmRemove;
                }
            }
        }
    }

    /// The text the install prompt takes.
    fn type_path(&mut self, code: KeyCode) {
        match code {
            keys::CANCEL => self.mode = Mode::Listing,
            keys::ERASE => match self.mode {
                Mode::Repository => {
                    self.repository.pop();
                }
                Mode::Package => {
                    self.package.pop();
                }
                Mode::Listing | Mode::ConfirmRemove => {}
            },
            keys::SUBMIT if self.mode == Mode::Repository => {
                if !self.repository.trim().is_empty() {
                    self.mode = Mode::Package;
                }
            }
            keys::SUBMIT => {
                let url = self.repository.trim().to_owned();
                let package = self.package.trim();
                self.pending_action = Some(ComponentAction::View(ViewAction::Plugin(
                    PluginOperation::Install {
                        url,
                        package: (!package.is_empty()).then(|| package.to_owned()),
                    },
                )));
                self.mode = Mode::Listing;
            }
            KeyCode::Char(character) => match self.mode {
                Mode::Repository => self.repository.push(character),
                Mode::Package => self.package.push(character),
                Mode::Listing | Mode::ConfirmRemove => {}
            },
            _ => {}
        }
    }

    fn update_selected(&mut self, model: &UiModel) {
        if let Some(plugin) = model.plugins.get(self.selected) {
            self.pending_action = Some(ComponentAction::View(ViewAction::Plugin(
                PluginOperation::Update {
                    name: Some(plugin.name.clone()),
                },
            )));
        }
    }

    fn toggle_selected(&mut self, model: &UiModel) {
        if let Some(plugin) = model.plugins.get(self.selected) {
            let operation = if plugin.enabled {
                PluginOperation::Disable {
                    name: plugin.name.clone(),
                }
            } else {
                PluginOperation::Enable {
                    name: plugin.name.clone(),
                }
            };
            self.pending_action = Some(ComponentAction::View(ViewAction::Plugin(operation)));
        }
    }
}

/// The step a long operation reached, as a bar and a caption.
///
/// A build can take minutes, so the page shows the step count, the stage and a
/// filled bar: the reader can tell it is working and how far along it is.
fn progress_line(job: &crate::ui_model::PluginJob, model: &UiModel, width: u16) -> Line<'static> {
    let theme = model.preferences.theme;
    // Integer arithmetic: the bar divides the cells over the steps, so a
    // three-of-four build fills three of four cells without rounding drift.
    let total = job.total.max(1);
    let cells = width.saturating_sub(48).clamp(6, 24);
    let scaled = job.step.min(total).saturating_mul(u64::from(cells));
    let filled = u16::try_from(scaled.div_ceil(total))
        .unwrap_or(cells)
        .min(cells);
    Line::from(vec![
        Span::styled(" ", Style::default()),
        Span::styled(
            format!(" {} ", job.operation),
            Style::default()
                .fg(palette::background(theme))
                .bg(palette::accent(theme))
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            "█".repeat(usize::from(filled)),
            Style::default().fg(palette::accent(theme)),
        ),
        Span::styled(
            "░".repeat(usize::from(cells - filled)),
            Style::default().fg(palette::border(theme)),
        ),
        Span::styled(
            format!(" {}/{}  {}", job.step, job.total, job.stage),
            palette::muted_style(theme),
        ),
    ])
}

/// Which toolbar button sits under a column.
///
/// The toolbar draws one cell, then each button as " label " and a space, so
/// the hit test walks the same widths instead of guessing them.
fn action_at(toolbar: Rect, column: u16, row: u16) -> Option<usize> {
    if row != toolbar.y {
        return None;
    }
    let mut start = toolbar.x.saturating_add(1);
    for (index, action) in ACTIONS.iter().enumerate() {
        let width = u16::try_from(action.label().chars().count() + 2).unwrap_or(0);
        if column >= start && column < start.saturating_add(width) {
            return Some(index);
        }
        start = start.saturating_add(width).saturating_add(1);
    }
    None
}

#[cfg(test)]
#[path = "plugins_tests.rs"]
mod tests;
