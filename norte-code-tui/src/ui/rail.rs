//! The rail: session identity, connection state and context usage.
//!
//! The context section owns a gauge and a compact button as children, so
//! the tree routes focus and clicks to them like any other view.

use std::any::Any;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use serde::{Deserialize, Serialize};

use terminalui::components::button::{Button, ButtonVariant};
use terminalui::components::chrome::{Border, BorderSet, Chrome, Padding, Radius, draw_box};
use terminalui::components::gauge::{Gauge, GaugeStyle};
use terminalui::{Context, InputEvent, Node, Outcome, Theme, View, node};

use crate::context::{ContextUsage, compaction_threshold, human_count};
use crate::ui_model::SessionAttachmentState;

const RAIL_WIDTH: u16 = 26;

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
    /// The provider id, when the host reported one.
    pub(crate) provider: Option<String>,
    /// The model id, when the host reported one.
    pub(crate) model: Option<String>,
    /// The context the session last reported.
    context: ContextUsage,
    /// Background jobs of the attached session.
    jobs: Vec<norte_code_wire::JobSummary>,
    /// Plans of the attached session.
    plans: Vec<norte_code_wire::PlanSummary>,
    /// Clickable plan rows, recorded during render.
    plan_rows: Vec<(Rect, String)>,
    /// A plan the user clicked, waiting for the host to open it.
    pending_plan: Option<String>,
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
            provider: None,
            model: None,
            context: ContextUsage::default(),
            jobs: Vec::new(),
            plans: Vec::new(),
            plan_rows: Vec::new(),
            pending_plan: None,
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

    /// Replace the context the gauge reports.
    pub(crate) fn set_context(&mut self, context: ContextUsage) {
        self.context = context;
    }

    pub(crate) fn set_jobs(&mut self, jobs: Vec<norte_code_wire::JobSummary>) {
        self.jobs = jobs;
    }

    pub(crate) fn set_plans(&mut self, plans: Vec<norte_code_wire::PlanSummary>) {
        self.plans = plans;
    }

    /// Take a plan request from a click.
    pub(crate) fn take_plan_request(&mut self) -> Option<String> {
        self.pending_plan.take()
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

    fn paint_context(
        &mut self,
        inner: Rect,
        y: u16,
        buf: &mut Buffer,
        cx: &mut Context,
        theme: &Theme,
    ) -> u16 {
        let usage = self.context;
        let start = y + 1;
        let bottom = inner.y + inner.height;
        let room = start + 3 < bottom;
        if !room {
            // No space to draw, but the tree still dispatches focus to the
            // button; advance the slot here too so both walks agree.
            let focusable = self
                .children
                .get(1)
                .and_then(|node| node.view.as_any_ref().downcast_ref::<Button>())
                .is_some_and(|button| !button.state.disabled && !button.state.loading);
            if focusable {
                let _ = cx.is_focused();
            }
            return bottom;
        }
        let dim = Style::default().fg(theme.muted).bg(theme.background);
        let color = if usage.over_limit() {
            theme.warning
        } else {
            theme.foreground
        };

        buf.set_stringn(
            inner.x + 1,
            start,
            "CONTEXT",
            8,
            dim.add_modifier(Modifier::BOLD),
        );
        let (used, limit) = match usage.window.filter(|window| *window > 0) {
            Some(window) => (human_count(usage.used), human_count(window)),
            None => (human_count(usage.used), "?".to_owned()),
        };
        let counts = format!("{used} / {limit}");
        buf.set_stringn(
            inner.x + 1,
            start + 2,
            &counts,
            counts.chars().count(),
            Style::default().fg(color).bg(theme.background),
        );
        let compact = if usage.window.is_some() {
            format!("compact at {}", human_count(usage.compaction_limit()))
        } else {
            "compact at ?".to_owned()
        };
        buf.set_stringn(
            inner.x + 1,
            start + 3,
            &compact,
            compact.chars().count(),
            Style::default().fg(theme.muted).bg(theme.background),
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
        }
        if let Some(node) = self.children.get_mut(1) {
            node.view.render(button_rect, buf, cx, theme);
        }
        start + 4
    }
    fn paint_plans(&mut self, buf: &mut Buffer, inner: Rect, y: u16, theme: &Theme) {
        self.plan_rows.clear();
        if y + 2 >= inner.y + inner.height {
            return;
        }
        let dim = Style::default().fg(theme.muted).bg(theme.background);
        buf.set_stringn(inner.x + 1, y, "PLANS", 6, dim.add_modifier(Modifier::BOLD));
        if self.plans.is_empty() {
            buf.set_stringn(inner.x + 1, y + 1, "none", 4, dim);
            return;
        }
        for (row, plan) in (y + 1..).zip(self.plans.iter().take(3)) {
            if row >= inner.y + inner.height {
                break;
            }
            let title = plan.title.clone().unwrap_or_else(|| plan.id.clone());
            buf.set_stringn(
                inner.x + 1,
                row,
                &title,
                usize::from(inner.width.saturating_sub(2)),
                Style::default().fg(theme.accent).bg(theme.background),
            );
            self.plan_rows.push((
                Rect {
                    x: inner.x,
                    y: row,
                    width: inner.width,
                    height: 1,
                },
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
        let chrome = Chrome {
            border: Some(Border {
                set: BorderSet::Rounded,
                color: Some(theme.border),
                radius: Radius::Small,
            }),
            background: None,
            radius: Radius::Small,
            padding: Padding {
                horizontal: 0,
                vertical: 0,
            },
            border_gradient: None,
            fill_gradient: None,
        };
        let inner = draw_box(buf, area, &chrome, theme);
        let mut y = inner.y + 1;

        if inner.width >= 10 {
            y = paint_session(buf, inner, y, self.session.as_deref(), theme);
            y = paint_model(
                buf,
                inner,
                y,
                self.model.as_deref(),
                self.provider.as_deref(),
                theme,
            );
            y = paint_connection(buf, inner, y, &self.attachment, theme);
        }
        y = self.paint_context(inner, y, buf, cx, theme);
        y = paint_jobs(buf, inner, y, &self.jobs, theme);
        self.paint_plans(buf, inner, y, theme);
    }

    fn handle_event(&mut self, event: &InputEvent, _cx: &mut Context) -> Outcome {
        if let InputEvent::Mouse(mouse) = event
            && mouse.kind
                == crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left)
        {
            for (rect, id) in &self.plan_rows {
                if rect.contains(ratatui::layout::Position::new(mouse.column, mouse.row)) {
                    self.pending_plan = Some(id.clone());
                    return Outcome::Consumed;
                }
            }
        }
        Outcome::Ignored
    }
}

fn paint_jobs(
    buf: &mut Buffer,
    inner: Rect,
    y: u16,
    jobs: &[norte_code_wire::JobSummary],
    theme: &Theme,
) -> u16 {
    if y + 2 >= inner.y + inner.height {
        return y;
    }
    let dim = Style::default().fg(theme.muted).bg(theme.background);
    buf.set_stringn(inner.x + 1, y, "JOBS", 5, dim.add_modifier(Modifier::BOLD));
    if jobs.is_empty() {
        buf.set_stringn(inner.x + 1, y + 1, "none", 4, dim);
        return y + 2;
    }
    let mut row = y + 1;
    for job in jobs.iter().take(3) {
        if row >= inner.y + inner.height {
            break;
        }
        let text = format!("{} {}", job.status.label(), clipped(&job.command, 18));
        buf.set_stringn(
            inner.x + 1,
            row,
            &text,
            usize::from(inner.width.saturating_sub(2)),
            Style::default().fg(theme.foreground).bg(theme.background),
        );
        row += 1;
    }
    row + 1
}

fn clipped(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

fn paint_session(
    buf: &mut Buffer,
    inner: Rect,
    y: u16,
    session: Option<&str>,
    theme: &Theme,
) -> u16 {
    let dim = Style::default().fg(theme.muted).bg(theme.background);
    let text = Style::default().fg(theme.foreground).bg(theme.background);
    buf.set_stringn(
        inner.x + 1,
        y,
        "SESSION",
        7,
        dim.add_modifier(Modifier::BOLD),
    );
    let y = y + 1;
    let usable = usize::from(inner.width.saturating_sub(2));
    if let Some(session) = session {
        let chars: Vec<char> = session.chars().collect();
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
        buf.set_stringn(inner.x + 1, y, "no session", 10, dim);
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

fn paint_connection(
    buf: &mut Buffer,
    inner: Rect,
    y: u16,
    attachment: &SessionAttachmentState,
    theme: &Theme,
) -> u16 {
    let dim = Style::default().fg(theme.muted).bg(theme.background);
    buf.set_stringn(
        inner.x + 1,
        y,
        "CONNECTION",
        10,
        dim.add_modifier(Modifier::BOLD),
    );
    let y = y + 1;
    match attachment {
        SessionAttachmentState::Disconnected => {
            buf.set_stringn(inner.x + 1, y, "detached", 8, dim);
        }
        SessionAttachmentState::Connecting => {
            buf.set_stringn(
                inner.x + 1,
                y,
                "● connecting…",
                14,
                Style::default().fg(theme.warning).bg(theme.background),
            );
        }
        SessionAttachmentState::Attached => {
            buf.set_stringn(
                inner.x + 1,
                y,
                "● attached",
                10,
                Style::default().fg(theme.success).bg(theme.background),
            );
        }
        SessionAttachmentState::Failed { reason } => {
            buf.set_stringn(
                inner.x + 1,
                y,
                "✗ failed",
                9,
                Style::default()
                    .fg(theme.destructive)
                    .bg(theme.background)
                    .add_modifier(Modifier::BOLD),
            );
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
