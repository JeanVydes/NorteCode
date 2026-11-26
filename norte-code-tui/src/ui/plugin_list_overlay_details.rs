use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
};
use terminalui::Theme;

use norte_code_client::PluginSummary;

pub(super) fn paint_details(
    buf: &mut Buffer,
    theme: &Theme,
    area: Rect,
    plugin: Option<&PluginSummary>,
    loading: bool,
    error: Option<&str>,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let Some(plugin) = plugin else {
        let text = error.unwrap_or(if loading {
            "Connecting to the service to load plugin details"
        } else {
            "Plugin details will appear here"
        });
        let color = if error.is_some() {
            theme.destructive
        } else {
            theme.muted
        };
        buf.set_stringn(
            area.x,
            area.y,
            text,
            usize::from(area.width),
            Style::default().fg(color),
        );
        return;
    };

    let mut row = 0;
    let origin = plugin.source.as_deref().unwrap_or("not recorded");
    write_wrapped(buf, area, row, "Origin", origin, theme.foreground);
    row += wrapped_rows(origin, area.width);

    let package = plugin.package.as_deref().unwrap_or("not recorded");
    write_wrapped(buf, area, row, "Package", package, theme.foreground);
    row += wrapped_rows(package, area.width);

    let state = format!(
        "{} / {}",
        if plugin.enabled {
            "enabled"
        } else {
            "disabled"
        },
        if plugin.loaded {
            "loaded"
        } else if plugin.load_error.is_some() {
            "failed to load"
        } else {
            "not loaded"
        }
    );
    write_wrapped(buf, area, row, "State", &state, theme.foreground);
    row += 1;
    if let Some(error) = plugin.load_error.as_deref() {
        write_wrapped(buf, area, row, "Load error", error, theme.destructive);
        row += wrapped_rows(error, area.width);
    }

    let tools = list_or_none(&plugin.tools);
    write_wrapped(buf, area, row, "Tools", &tools, theme.foreground);
    row += wrapped_rows(&tools, area.width);

    let commands = plugin
        .commands
        .iter()
        .map(|command| format!("/{}", command.name))
        .collect::<Vec<_>>();
    let commands = list_or_none(&commands);
    write_wrapped(buf, area, row, "Commands", &commands, theme.foreground);
    row += wrapped_rows(&commands, area.width);

    let widgets = plugin
        .widgets
        .iter()
        .map(|widget| format!("{} ({})", widget.title, widget.key.widget.as_str()))
        .collect::<Vec<_>>();
    let widgets = list_or_none(&widgets);
    write_wrapped(buf, area, row, "Widgets", &widgets, theme.foreground);
}

fn list_or_none(values: &[String]) -> String {
    if values.is_empty() {
        "none declared".to_owned()
    } else {
        values.join(", ")
    }
}

fn wrapped_rows(value: &str, width: u16) -> usize {
    let available = usize::from(width).saturating_sub(label_width(width)).max(1);
    value.chars().count().max(1).div_ceil(available)
}

fn label_width(width: u16) -> usize {
    12usize.min(usize::from(width) / 3)
}

fn write_wrapped(
    buf: &mut Buffer,
    area: Rect,
    row: usize,
    label: &str,
    value: &str,
    color: ratatui::style::Color,
) {
    let label_width = label_width(area.width);
    let value_width = usize::from(area.width).saturating_sub(label_width).max(1);
    let mut current_row = row;
    let mut line = String::new();
    for character in value.chars() {
        if line.chars().count() >= value_width {
            paint_detail_line(buf, area, current_row, label, &line, label_width, color);
            current_row += 1;
            line.clear();
        }
        line.push(character);
    }
    paint_detail_line(buf, area, current_row, label, &line, label_width, color);
}

fn paint_detail_line(
    buf: &mut Buffer,
    area: Rect,
    row: usize,
    label: &str,
    value: &str,
    label_width: usize,
    color: ratatui::style::Color,
) {
    let Ok(row) = u16::try_from(row) else {
        return;
    };
    if row >= area.height {
        return;
    }
    let y = area.y + row;
    buf.set_stringn(
        area.x,
        y,
        label,
        label_width,
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    );
    buf.set_stringn(
        area.x + u16::try_from(label_width).unwrap_or(u16::MAX),
        y,
        value,
        usize::from(area.width).saturating_sub(label_width),
        Style::default().fg(color),
    );
}
