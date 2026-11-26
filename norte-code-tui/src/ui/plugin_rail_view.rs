//! Places plugin widgets in the terminal rail and adapts them to its available space.

use chrono::{DateTime, Local};
use norte_code_wire::plugin_widgets::{BadgeTone, Widget, WidgetUpdate};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
};
use terminalui::components::badge::{Badge, BadgeVariant};
use terminalui::components::progress::Progress;
use terminalui::components::sparkline::Sparkline;
use terminalui::components::table::{Table, TableCell, TableRow};
use terminalui::{Theme, Tree, node};

const SECTION_SPACING: u16 = 1;
const MAX_ITEMS: usize = 8;
const MAX_ROWS: u16 = 64;

pub(super) fn height(updates: &[WidgetUpdate]) -> u16 {
    updates
        .iter()
        .fold(0u16, |sum, update| {
            let title = u16::from(update.view.title.is_some());
            let content = update
                .view
                .content
                .iter()
                .map(widget_height)
                .fold(0u16, u16::saturating_add);
            sum.saturating_add(SECTION_SPACING + title)
                .saturating_add(content)
        })
        .min(MAX_ROWS)
}

fn widget_height(widget: &Widget) -> u16 {
    let count = |len: usize| u16::try_from(len.min(MAX_ITEMS)).unwrap_or(0);
    match widget {
        Widget::Text { .. } | Widget::Badge { .. } | Widget::Divider => 1,
        Widget::Progress { .. } => 2,
        Widget::Sparkline { label, .. } => 1 + u16::from(label.is_some()),
        Widget::List { items } => count(items.len()),
        Widget::KeyValue { fields } => count(fields.len()),
        Widget::Table { columns, records } => u16::from(!columns.is_empty()) + count(records.len()),
    }
}

pub(super) fn paint(
    updates: &[WidgetUpdate],
    buf: &mut Buffer,
    area: Rect,
    mut y: u16,
    theme: &Theme,
) -> u16 {
    let width = area.width.saturating_sub(2);
    let bottom = (y.saturating_add(height(updates))).min(area.bottom());
    if width == 0 {
        return y;
    }
    let x = area.x + 1;
    let text = Style::default().fg(theme.foreground).bg(theme.background);
    let heading = text.add_modifier(Modifier::BOLD);
    for update in updates {
        if y + SECTION_SPACING >= bottom {
            break;
        }
        y += SECTION_SPACING;
        if let Some(title) = &update.view.title {
            line(buf, Rect::new(x, y, width, 1), title, heading);
            y += 1;
        }
        for widget in &update.view.content {
            let height = widget_height(widget);
            if height > bottom - y {
                return bottom;
            }
            if height > 0 {
                paint_widget(widget, buf, Rect::new(x, y, width, height), text, theme);
                y += height;
            }
        }
    }
    y
}

fn paint_widget(widget: &Widget, buf: &mut Buffer, area: Rect, style: Style, theme: &Theme) {
    match widget {
        Widget::Text { text } => line(buf, area, text, style),
        Widget::Progress {
            label,
            percent,
            resets_at,
        } => {
            paint_progress(buf, area, style, theme, label, *percent, *resets_at);
        }
        Widget::Badge { text, tone } => {
            let variant = match tone {
                BadgeTone::Neutral => BadgeVariant::Neutral,
                BadgeTone::Success => BadgeVariant::Success,
                BadgeTone::Warning => BadgeVariant::Warning,
                BadgeTone::Error => BadgeVariant::Destructive,
            };
            let badge = Badge::new(printable(text)).with_variant(variant);
            Tree::new(node("badge", badge)).render(area, buf, theme);
        }
        Widget::List { items } => {
            paint_lines(
                buf,
                area,
                style,
                items.iter().map(|item| format!("• {item}")),
            );
        }
        Widget::KeyValue { fields } => {
            paint_lines(
                buf,
                area,
                style,
                fields
                    .iter()
                    .map(|field| format!("{}: {}", field.label, field.value)),
            );
        }
        Widget::Table { columns, records } => paint_table(columns, records, buf, area, theme),
        Widget::Sparkline { label, samples } => {
            let mut plot = area;
            if let Some(label) = label {
                line(buf, area, label, style);
                plot.y += 1;
                plot.height -= 1;
            }
            let samples = samples
                .iter()
                .rev()
                .copied()
                .filter(|sample| sample.is_finite())
                .take(usize::from(area.width))
                .collect::<Vec<_>>();
            let sparkline = Sparkline::new(samples.into_iter().rev().collect());
            Tree::new(node("sparkline", sparkline)).render(plot, buf, theme);
        }
        Widget::Divider => line(
            buf,
            area,
            &"─".repeat(usize::from(area.width)),
            style.fg(theme.border),
        ),
    }
}

fn paint_progress(
    buf: &mut Buffer,
    area: Rect,
    style: Style,
    theme: &Theme,
    label: &str,
    percent: u8,
    resets_at: Option<i64>,
) {
    let percent = percent.min(100);
    let mut caption = format!("{} {percent}%", printable(label));
    if let Some(reset) = resets_at.and_then(|seconds| DateTime::from_timestamp(seconds, 0)) {
        use std::fmt::Write as _;
        let _ = write!(
            caption,
            " · {}",
            reset.with_timezone(&Local).format("%a %H:%M")
        );
    }
    line(buf, area, &caption, style);
    let bar = Progress::new(f64::from(percent) / 100.0);
    Tree::new(node("progress", bar)).render(
        Rect {
            y: area.y + 1,
            height: 1,
            ..area
        },
        buf,
        theme,
    );
}

fn paint_table(
    columns: &[String],
    records: &[Vec<String>],
    buf: &mut Buffer,
    area: Rect,
    theme: &Theme,
) {
    let columns_visible = usize::from((area.width / 5).max(1)).min(MAX_ITEMS);
    let cells = |values: &[String]| {
        values
            .iter()
            .take(columns_visible)
            .map(|value| node("cell", TableCell::text(printable(value))))
            .collect()
    };
    let mut table = Table::new();
    if !columns.is_empty() {
        table = table.with_header(node("header", TableRow::header(cells(columns))));
    }
    table.set_rows(
        records
            .iter()
            .take(MAX_ITEMS)
            .map(|record| node("record", TableRow::body(cells(record))))
            .collect(),
    );
    Tree::new(node("table", table)).render(area, buf, theme);
}

fn paint_lines(buf: &mut Buffer, area: Rect, style: Style, lines: impl Iterator<Item = String>) {
    for (y, text) in (area.y..area.bottom()).zip(lines) {
        line(buf, Rect { y, ..area }, &text, style);
    }
}

fn line(buf: &mut Buffer, area: Rect, text: &str, style: Style) {
    buf.set_stringn(
        area.x,
        area.y,
        printable(text),
        usize::from(area.width),
        style,
    );
}

fn printable(text: &str) -> String {
    text.chars()
        .filter(|character| !character.is_control())
        .collect()
}
