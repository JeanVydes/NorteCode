//! The rail: session identity, connection state and context usage.
//!
//! The context section owns a gauge and a compact button as children, so
//! the tree routes focus and clicks to them like any other view.

use std::any::Any;

use crossterm::event::MouseEventKind;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use serde::{Deserialize, Serialize};

use terminalui::components::badge::{Badge, BadgeVariant};
use terminalui::components::button::{Button, ButtonVariant};
use terminalui::components::chrome::{Chrome, Padding, draw_box};
use terminalui::components::gauge::{Gauge, GaugeStyle};
use terminalui::components::stat::Stat;
use terminalui::{Context, InputEvent, Node, Outcome, Theme, View, node};

use crate::context::{ContextUsage, compaction_threshold, human_count};
use crate::ui::copy::{
    ATTACHED, COMPACT_AT, CONNECTING_DOT, CONNECTION, CONTEXT, DETACHED, FAILED, NO_SESSION, NONE,
    PLANS, PLUGIN_BANNER, PLUGIN_STATUS, SESSION, TODO_COMPLETED, TODO_IN_PROGRESS, TODO_PENDING,
    TODOS,
};
use crate::ui_model::SessionAttachmentState;

const RAIL_WIDTH: u16 = 26;
const PREVIEW_PREFIX: &str = "preview_";

fn preview_id(index: usize) -> String {
    format!("{PREVIEW_PREFIX}{index}")
}

/// Where virtual rail rows land on screen.
struct RailViewport {
    /// The rail's inner rectangle.
    inner: Rect,
    /// Rows scrolled out of view.
    scroll: u16,
}

/// Where the session rail sits relative to the conversation.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RailPosition {
    /// No rail.
    Hide,
    /// Left of the stream.
    #[default]
    Left,
    /// Right of the stream.
    Right,
    /// Above the stream, below the tabs.
    Top,
    /// Below the stream, above the composer.
    Bottom,
}

impl RailPosition {
    /// The label shown in the config page.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Hide => "hide",
            Self::Left => "left",
            Self::Right => "right",
            Self::Top => "top",
            Self::Bottom => "bottom",
        }
    }

    /// Read the label the settings page reports.
    #[cfg(test)]
    pub(crate) fn from_label(label: &str) -> Option<Self> {
        match label {
            "hide" => Some(Self::Hide),
            "left" => Some(Self::Left),
            "right" => Some(Self::Right),
            "top" => Some(Self::Top),
            "bottom" => Some(Self::Bottom),
            _ => None,
        }
    }

    /// The next position in the cycle.
    pub(crate) const fn next(self) -> Self {
        match self {
            Self::Hide => Self::Left,
            Self::Left => Self::Right,
            Self::Right => Self::Top,
            Self::Top => Self::Bottom,
            Self::Bottom => Self::Hide,
        }
    }
}

/// The rail, with the context gauge and compact button as children.
#[derive(Serialize)]
pub(crate) struct Rail {
    /// Where the connection stands right now.
    pub(crate) attachment: SessionAttachmentState,
    /// The session id, when one exists.
    pub(crate) session: Option<String>,
    /// The session title, when the host reported one.
    pub(crate) title: Option<String>,
    /// The provider id, when the host reported one.
    pub(crate) provider: Option<String>,
    /// The model id, when the host reported one.
    pub(crate) model: Option<String>,
    /// The context the session last reported.
    context: ContextUsage,
    /// Plans of the attached session.
    plans: Vec<norte_code_wire::PlanSummary>,
    /// The task list of the attached session.
    todos: Vec<norte_code_wire::TodoInfo>,
    /// Status lines offered by loaded plugins.
    plugin_status: Vec<String>,
    /// Banner lines offered by loaded plugins.
    plugin_banner: Vec<String>,
    /// Clickable plan rows, recorded during render.
    plan_rows: Vec<(Rect, String)>,
    /// A plan the user clicked, waiting for the host to open it.
    pending_plan: Option<String>,
    /// Whether the next render should bring the plans section into view.
    reveal_plans: bool,
    /// Whether the next render should bring the todo section into view.
    reveal_todos: bool,
    /// How many virtual rows are scrolled out of view.
    scroll: u16,
    /// The last rendered frame, for mouse hit-testing.
    area: Rect,
    inner: Rect,
    children: Vec<Node>,
}

impl Rail {
    /// Creates a rail for a never-attached session.
    pub(crate) fn detached() -> Self {
        let gauge = Gauge::new(0.0).with_style(GaugeStyle {
            warn_above: Some(compaction_threshold()),
            ..GaugeStyle::default()
        });
        Self {
            attachment: SessionAttachmentState::Disconnected,
            session: None,
            title: None,
            provider: None,
            model: None,
            context: ContextUsage::default(),
            plans: Vec::new(),
            todos: Vec::new(),
            plugin_status: Vec::new(),
            plugin_banner: Vec::new(),
            plan_rows: Vec::new(),
            pending_plan: None,
            reveal_plans: false,
            reveal_todos: false,
            scroll: 0,
            area: Rect::new(0, 0, 0, 0),
            inner: Rect::new(0, 0, 0, 0),
            children: vec![
                node("context_gauge", gauge),
                node("compact", Button::new(ButtonVariant::Outline, "compact")),
            ],
        }
    }

    /// Replace the session shown; `None` clears the id line.
    pub(crate) fn set_session(&mut self, session: Option<String>) {
        self.session = session;
    }

    /// Replace the title shown; `None` falls back to the session id.
    pub(crate) fn set_title(&mut self, title: Option<String>) {
        self.title = title;
    }

    /// The context the gauge currently reports.
    pub(crate) fn context(&self) -> ContextUsage {
        self.context
    }

    /// Replace the context the gauge reports.
    pub(crate) fn set_context(&mut self, context: ContextUsage) {
        self.context = context;
    }

    /// The plans currently shown by the rail.
    pub(crate) fn plans(&self) -> &[norte_code_wire::PlanSummary] {
        &self.plans
    }

    pub(crate) fn set_plans(&mut self, plans: Vec<norte_code_wire::PlanSummary>) {
        let revealed = self.plans.is_empty() && !plans.is_empty();
        self.sync_plan_buttons(plans.len());
        self.plans = plans;
        self.reveal_plans |= revealed;
    }

    /// The task list currently shown by the rail.
    pub(crate) fn todos(&self) -> &[norte_code_wire::TodoInfo] {
        &self.todos
    }

    pub(crate) fn set_todos(&mut self, todos: Vec<norte_code_wire::TodoInfo>) {
        let revealed = self.todos.is_empty() && !todos.is_empty();
        self.todos = todos;
        self.reveal_todos |= revealed;
    }

    /// The status lines currently shown by the rail.
    pub(crate) fn plugin_status(&self) -> &[String] {
        &self.plugin_status
    }

    pub(crate) fn set_plugin_status(&mut self, lines: Vec<String>) {
        self.plugin_status = lines;
    }

    /// The banner lines currently shown by the rail.
    pub(crate) fn plugin_banner(&self) -> &[String] {
        &self.plugin_banner
    }

    pub(crate) fn set_plugin_banner(&mut self, lines: Vec<String>) {
        self.plugin_banner = lines;
    }

    fn sync_plan_buttons(&mut self, count: usize) {
        let expected = 2 + count;
        self.children.truncate(expected);
        while self.children.len() < expected {
            let index = self.children.len() - 2;
            self.children.push(node(
                preview_id(index),
                Button::new(ButtonVariant::Outline, "open"),
            ));
        }
    }

    /// Take a plan request from a click or a row's preview button.
    pub(crate) fn take_plan_request(&mut self) -> Option<String> {
        let Self {
            children,
            plans,
            pending_plan,
            ..
        } = self;
        let clicked = pending_plan.take();
        let previewed = children
            .iter_mut()
            .skip(2)
            .enumerate()
            .find_map(|(index, child)| {
                child
                    .view
                    .as_any()
                    .downcast_mut::<Button>()
                    .is_some_and(Button::take_pressed)
                    .then(|| plans.get(index).map(|plan| plan.id.clone()))
                    .flatten()
            });
        previewed.or(clicked)
    }

    /// Whether the compact button is currently enabled.
    pub(crate) fn compact_enabled(&self) -> bool {
        self.children
            .get(1)
            .and_then(|node| node.view.as_any_ref().downcast_ref::<Button>())
            .is_some_and(|button| !button.state.disabled)
    }

    /// Enable or disable the compact button from the host's state.
    pub(crate) fn set_compact_enabled(&mut self, enabled: bool) {
        if let Some(button) = self
            .children
            .get_mut(1)
            .and_then(|node| node.view.as_any().downcast_mut::<Button>())
        {
            button.set_disabled(!enabled);
        }
    }

    /// Rows the whole rail would need without clipping.
    fn content_height(&self, width: u16) -> u16 {
        let usable = usize::from(width.saturating_sub(2));
        let mut height = 0u16;
        if width >= 10 {
            height += match &self.session {
                Some(session) if session.chars().count() > usable => 3,
                Some(_) => 2,
                None => 3,
            };
            height += 3;
            height += if matches!(self.attachment, SessionAttachmentState::Failed { .. }) {
                4
            } else {
                3
            };
        }
        height += 5;
        if !self.plugin_status.is_empty() {
            height += section_rows(self.plugin_status.len());
        }
        if !self.plugin_banner.is_empty() {
            height += section_rows(self.plugin_banner.len());
        }
        if !self.todos.is_empty() {
            height += section_rows(self.todos.len());
        }
        height += section_rows(self.plans.len());
        height
    }

    fn to_screen(rect: Rect, viewport: &RailViewport) -> Rect {
        Rect {
            x: viewport.inner.x + rect.x,
            y: viewport.inner.y + 1 + rect.y.saturating_sub(viewport.scroll),
            ..rect
        }
    }

    fn paint_context(
        &mut self,
        inner: Rect,
        y: u16,
        buf: &mut Buffer,
        cx: &mut Context,
        theme: &Theme,
        viewport: &RailViewport,
    ) -> u16 {
        let usage = self.context;
        let start = y + 1;
        let dim = Style::default().fg(theme.muted).bg(theme.background);

        buf.set_stringn(
            inner.x + 1,
            start,
            CONTEXT,
            8,
            dim.add_modifier(Modifier::BOLD),
        );
        let (used, limit) = match usage.window.filter(|window| *window > 0) {
            Some(window) => (human_count(usage.used), human_count(window)),
            None => (human_count(usage.used), "?".to_owned()),
        };
        let counts = format!("{used} / {limit}");
        let compact = if usage.window.is_some() {
            format!("{COMPACT_AT} {}", human_count(usage.compaction_limit()))
        } else {
            format!("{COMPACT_AT} ?")
        };
        let mut metric = Stat::new(compact, counts);
        if usage.over_limit() {
            metric.style.value = Some(theme.warning);
        }
        metric.render(
            Rect {
                x: inner.x + 1,
                y: start + 2,
                width: inner.width.saturating_sub(2),
                height: 2,
            },
            buf,
            cx,
            theme,
        );

        let bar_y = start + 1;
        let gauge_rect = Rect {
            x: inner.x + 1,
            y: bar_y,
            width: 12,
            height: 1,
        };
        let button_rect = Rect {
            x: gauge_rect.x + 13,
            y: bar_y,
            width: 9,
            height: 1,
        };
        if let Some(node) = self.children.first_mut() {
            if let Some(gauge) = node.view.as_any().downcast_mut::<Gauge>() {
                gauge.value = usage.fraction();
            }
            node.view.render(gauge_rect, buf, cx, theme);
            if let Some(gauge) = node.view.as_any().downcast_mut::<Gauge>() {
                gauge.state.area = Self::to_screen(gauge_rect, viewport);
            }
        }
        if let Some(node) = self.children.get_mut(1) {
            node.view.render(button_rect, buf, cx, theme);
            if let Some(button) = node.view.as_any().downcast_mut::<Button>() {
                button.state.area = Self::to_screen(button_rect, viewport);
            }
        }
        start + 4
    }

    /// Paints a titled list of plain lines, clipped to the rail width.
    fn paint_lines(
        buf: &mut Buffer,
        inner: Rect,
        y: u16,
        theme: &Theme,
        title: &str,
        lines: &[String],
    ) -> u16 {
        if lines.is_empty() || y + 1 >= inner.y + inner.height {
            return y;
        }
        let dim = Style::default().fg(theme.muted).bg(theme.background);
        buf.set_stringn(
            inner.x + 1,
            y,
            title,
            title.len(),
            dim.add_modifier(Modifier::BOLD),
        );
        let width = usize::from(inner.width.saturating_sub(3));
        let mut row = y + 1;
        for line in lines {
            if row >= inner.y + inner.height {
                break;
            }
            buf.set_stringn(inner.x + 1, row, clipped(line, width), width, dim);
            row += 1;
        }
        row
    }

    fn paint_plugin_status(&self, buf: &mut Buffer, inner: Rect, y: u16, theme: &Theme) -> u16 {
        Self::paint_lines(buf, inner, y, theme, PLUGIN_STATUS, &self.plugin_status)
    }

    fn paint_plugin_banner(&self, buf: &mut Buffer, inner: Rect, y: u16, theme: &Theme) -> u16 {
        Self::paint_lines(buf, inner, y, theme, PLUGIN_BANNER, &self.plugin_banner)
    }

    fn paint_todos(&self, buf: &mut Buffer, inner: Rect, y: u16, theme: &Theme) -> u16 {
        if self.todos.is_empty() || y + 1 >= inner.y + inner.height {
            return y;
        }
        let dim = Style::default().fg(theme.muted).bg(theme.background);
        buf.set_stringn(inner.x + 1, y, TODOS, 6, dim.add_modifier(Modifier::BOLD));
        let width = usize::from(inner.width.saturating_sub(3));
        let mut row = y + 1;
        for todo in &self.todos {
            if row >= inner.y + inner.height {
                break;
            }
            let marker = match todo.status {
                norte_code_wire::TodoStatus::Pending => TODO_PENDING,
                norte_code_wire::TodoStatus::InProgress => TODO_IN_PROGRESS,
                norte_code_wire::TodoStatus::Completed => TODO_COMPLETED,
            };
            let style = match todo.status {
                norte_code_wire::TodoStatus::Pending => dim,
                norte_code_wire::TodoStatus::InProgress => {
                    Style::default().fg(theme.accent).bg(theme.background)
                }
                norte_code_wire::TodoStatus::Completed => {
                    Style::default().fg(theme.success).bg(theme.background)
                }
            };
            let text = format!(
                "{marker} {}",
                clipped(&todo.content, width.saturating_sub(2))
            );
            buf.set_stringn(inner.x + 1, row, &text, width, style);
            row += 1;
        }
        row
    }

    fn paint_plans(
        &mut self,
        buf: &mut Buffer,
        inner: Rect,
        y: u16,
        theme: &Theme,
        viewport: &RailViewport,
        cx: &mut Context,
    ) {
        self.plan_rows.clear();
        if y + 1 >= inner.y + inner.height {
            return;
        }
        let dim = Style::default().fg(theme.muted).bg(theme.background);
        buf.set_stringn(inner.x + 1, y, PLANS, 6, dim.add_modifier(Modifier::BOLD));
        if self.plans.is_empty() {
            buf.set_stringn(inner.x + 1, y + 1, NONE, 4, dim);
            return;
        }
        for (index, (row, plan)) in (y + 1..).zip(self.plans.iter()).enumerate() {
            if row >= inner.y + inner.height {
                break;
            }
            let title = plan.title.clone().unwrap_or_else(|| plan.id.clone());
            buf.set_stringn(
                inner.x + 1,
                row,
                &title,
                usize::from(inner.width.saturating_sub(8)),
                Style::default().fg(theme.accent).bg(theme.background),
            );
            let button_rect = Rect {
                x: inner.x + inner.width.saturating_sub(7),
                y: row,
                width: 6,
                height: 1,
            };
            if let Some(node) = self.children.get_mut(2 + index) {
                node.view.render(button_rect, buf, cx, theme);
                if let Some(button) = node.view.as_any().downcast_mut::<Button>() {
                    button.state.area = Self::to_screen(button_rect, viewport);
                }
            }
            self.plan_rows.push((
                Self::to_screen(
                    Rect {
                        x: inner.x,
                        y: row,
                        width: inner.width.saturating_sub(7),
                        height: 1,
                    },
                    viewport,
                ),
                plan.id.clone(),
            ));
        }
    }
}

impl View for Rail {
    fn as_any(&mut self) -> &mut dyn Any {
        self
    }

    fn as_any_ref(&self) -> &dyn Any {
        self
    }

    fn kind(&self) -> &'static str {
        "rail"
    }

    fn measure(&self) -> Option<(u16, u16)> {
        Some((RAIL_WIDTH, 12))
    }

    fn children(&self) -> &[Node] {
        &self.children
    }

    fn children_mut(&mut self) -> &mut [Node] {
        &mut self.children
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut Context, theme: &Theme) {
        let chrome = Chrome::themed(
            theme,
            Some(theme.border),
            None,
            Padding {
                horizontal: 0,
                vertical: 0,
            },
        );
        let inner = draw_box(buf, area, &chrome, theme);
        self.area = area;
        self.inner = inner;

        let content_height = self.content_height(inner.width);
        let visible_rows = inner.height.saturating_sub(2);
        if self.reveal_plans || self.reveal_todos {
            self.scroll = content_height.saturating_sub(visible_rows);
            self.reveal_plans = false;
            self.reveal_todos = false;
        }
        self.scroll = self.scroll.min(content_height.saturating_sub(visible_rows));

        let content = Rect {
            x: 0,
            y: 0,
            width: inner.width,
            height: content_height.max(1),
        };
        let mut scratch = Buffer::empty(content);
        scratch.set_style(content, Style::default().bg(theme.background));
        let mut y = 0u16;

        if inner.width >= 10 {
            y = paint_session(
                &mut scratch,
                content,
                y,
                self.session.as_deref(),
                self.title.as_deref(),
                theme,
            );
            y = paint_model(
                &mut scratch,
                content,
                y,
                self.model.as_deref(),
                self.provider.as_deref(),
                theme,
            );
            y = paint_connection(&mut scratch, content, y, &self.attachment, cx, theme);
        }
        let viewport = RailViewport {
            inner,
            scroll: self.scroll,
        };
        y = self.paint_context(content, y, &mut scratch, cx, theme, &viewport);
        y = self.paint_plugin_status(&mut scratch, content, y, theme);
        y = self.paint_plugin_banner(&mut scratch, content, y, theme);
        y = self.paint_todos(&mut scratch, content, y, theme);
        self.paint_plans(&mut scratch, content, y, theme, &viewport, cx);

        for row in 0..visible_rows {
            let source = row.saturating_add(self.scroll);
            for column in 0..inner.width {
                let Some(cell) = scratch.cell(ratatui::layout::Position::new(column, source))
                else {
                    continue;
                };
                if let Some(target) = buf.cell_mut(ratatui::layout::Position::new(
                    inner.x + column,
                    inner.y + 1 + row,
                )) {
                    *target = cell.clone();
                }
            }
        }
    }

    fn handle_event(&mut self, event: &InputEvent, _cx: &mut Context) -> Outcome {
        let InputEvent::Mouse(mouse) = event else {
            return Outcome::Ignored;
        };
        let point = ratatui::layout::Position::new(mouse.column, mouse.row);
        if !self.area.contains(point) {
            return Outcome::Ignored;
        }
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.scroll = self.scroll.saturating_sub(3);
                Outcome::Consumed
            }
            MouseEventKind::ScrollDown => {
                let viewport = self.inner.height.saturating_sub(2);
                let max = self
                    .content_height(self.inner.width)
                    .saturating_sub(viewport);
                self.scroll = self.scroll.saturating_add(3).min(max);
                Outcome::Consumed
            }
            MouseEventKind::Down(crossterm::event::MouseButton::Left) => {
                for (rect, id) in &self.plan_rows {
                    if rect.contains(point) {
                        self.pending_plan = Some(id.clone());
                        return Outcome::Consumed;
                    }
                }
                Outcome::Ignored
            }
            _ => Outcome::Ignored,
        }
    }
}

fn section_rows(count: usize) -> u16 {
    if count == 0 {
        2
    } else {
        u16::try_from(count).unwrap_or(u16::MAX).saturating_add(1)
    }
}

fn clipped(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

fn paint_session(
    buf: &mut Buffer,
    inner: Rect,
    y: u16,
    session: Option<&str>,
    title: Option<&str>,
    theme: &Theme,
) -> u16 {
    let dim = Style::default().fg(theme.muted).bg(theme.background);
    let text = Style::default().fg(theme.foreground).bg(theme.background);
    buf.set_stringn(inner.x + 1, y, SESSION, 7, dim.add_modifier(Modifier::BOLD));
    let y = y + 1;
    let usable = usize::from(inner.width.saturating_sub(2));
    if let Some(label) = title.or(session) {
        let chars: Vec<char> = label.chars().collect();
        let first: String = chars.iter().take(usable).collect();
        buf.set_stringn(
            inner.x + 1,
            y,
            &first,
            first.chars().count(),
            text.add_modifier(Modifier::BOLD),
        );
        if chars.len() > usable {
            let second: String = chars.iter().skip(usable).take(usable).collect();
            buf.set_stringn(
                inner.x + 1,
                y + 1,
                &second,
                second.chars().count(),
                text.add_modifier(Modifier::BOLD),
            );
            y + 2
        } else {
            y + 1
        }
    } else {
        buf.set_stringn(inner.x + 1, y, NO_SESSION, 10, dim);
        y + 2
    }
}

fn paint_model(
    buf: &mut Buffer,
    inner: Rect,
    y: u16,
    model: Option<&str>,
    provider: Option<&str>,
    theme: &Theme,
) -> u16 {
    let dim = Style::default().fg(theme.muted).bg(theme.background);
    let usable = usize::from(inner.width.saturating_sub(2));
    if let Some(model) = model {
        let model = clipped(model, usable);
        buf.set_stringn(
            inner.x + 1,
            y,
            &model,
            model.chars().count(),
            Style::default().fg(theme.accent).bg(theme.background),
        );
    } else {
        buf.set_stringn(inner.x + 1, y, "—", 1, dim);
    }
    let y = y + 1;
    if let Some(provider) = provider {
        let provider = clipped(provider, usable);
        buf.set_stringn(inner.x + 1, y, &provider, provider.chars().count(), dim);
    } else {
        buf.set_stringn(inner.x + 1, y, "—", 1, dim);
    }
    y + 2
}

fn badge_row(inner: Rect, y: u16, label: &str) -> Rect {
    Rect {
        x: inner.x + 1,
        y,
        width: u16::try_from(label.chars().count() + 2).unwrap_or(u16::MAX),
        height: 1,
    }
}

fn paint_connection(
    buf: &mut Buffer,
    inner: Rect,
    y: u16,
    attachment: &SessionAttachmentState,
    cx: &mut Context,
    theme: &Theme,
) -> u16 {
    let dim = Style::default().fg(theme.muted).bg(theme.background);
    buf.set_stringn(
        inner.x + 1,
        y,
        CONNECTION,
        10,
        dim.add_modifier(Modifier::BOLD),
    );
    let y = y + 1;
    match attachment {
        SessionAttachmentState::Disconnected => Badge::new(DETACHED)
            .with_variant(BadgeVariant::Neutral)
            .render(badge_row(inner, y, DETACHED), buf, cx, theme),
        SessionAttachmentState::Connecting => Badge::new(CONNECTING_DOT)
            .with_variant(BadgeVariant::Warning)
            .render(badge_row(inner, y, CONNECTING_DOT), buf, cx, theme),
        SessionAttachmentState::Ready => Badge::new("ready")
            .with_variant(BadgeVariant::Success)
            .render(badge_row(inner, y, "ready"), buf, cx, theme),
        SessionAttachmentState::Attached => Badge::new(ATTACHED)
            .with_variant(BadgeVariant::Success)
            .render(badge_row(inner, y, ATTACHED), buf, cx, theme),
        SessionAttachmentState::Failed { reason } => {
            Badge::new(FAILED)
                .with_variant(BadgeVariant::Destructive)
                .render(badge_row(inner, y, FAILED), buf, cx, theme);
            let detail: String = reason
                .chars()
                .take(usize::from(inner.width.saturating_sub(3)))
                .collect();
            buf.set_stringn(inner.x + 1, y + 1, &detail, detail.len(), dim);
            return y + 3;
        }
    }
    y + 2
}
