use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
};
use terminalui::Theme;
use terminalui::components::chrome::{Border, BorderSet, Chrome, Padding, Radius, draw_box};

use self::plugin_list_overlay_details::paint_details;
use super::PluginListOverlay;

#[path = "plugin_list_overlay_details.rs"]
mod plugin_list_overlay_details;

pub(super) fn paint(overlay: &mut PluginListOverlay, area: Rect, buf: &mut Buffer, theme: &Theme) {
    if !overlay.open {
        return;
    }
    overlay.area = area;
    let width = area.width.saturating_sub(4).min(108);
    let height = area.height.saturating_sub(2).min(34);
    if width < 36 || height < 7 {
        return;
    }
    let rect = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    let pulse = (overlay.elapsed.as_secs_f64() * 1.5)
        .sin()
        .mul_add(0.12, 0.12);
    let chrome = Chrome {
        border: Some(Border {
            set: BorderSet::Rounded,
            color: Some(terminalui::lerp_color(theme.border, theme.accent, pulse)),
            radius: Radius::Small,
        }),
        background: Some(theme.secondary),
        radius: Radius::Small,
        padding: Padding {
            horizontal: 1,
            vertical: 0,
        },
        border_gradient: None,
        fill_gradient: None,
    };
    let inner = draw_box(buf, rect, &chrome, theme);
    let title = if overlay.loading {
        format!(" PLUGINS | refreshing {} installed ", overlay.plugins.len())
    } else {
        format!(" PLUGINS | {} installed ", overlay.plugins.len())
    };
    buf.set_stringn(
        inner.x,
        inner.y,
        title,
        usize::from(inner.width),
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD),
    );

    let body_y = inner.y.saturating_add(2);
    let body_height = inner.height.saturating_sub(4);
    let list_width = (inner.width.saturating_mul(38) / 100)
        .clamp(18, 34)
        .min(inner.width.saturating_sub(14));
    let detail_x = inner.x.saturating_add(list_width).saturating_add(2);
    let detail_width = inner.right().saturating_sub(detail_x);
    overlay.list_area = Rect::new(inner.x, body_y, list_width, body_height);
    if body_height > 0 {
        paint_list(overlay, buf, theme);
        paint_details(
            buf,
            theme,
            Rect::new(detail_x, body_y, detail_width, body_height),
            overlay.plugins.get(overlay.selected),
            overlay.loading,
            overlay.error.as_deref(),
        );
    }
    let footer = if overlay.loading {
        "Up/Down select | PgUp/PgDn scroll | Esc close | refreshing"
    } else {
        "Up/Down select | PgUp/PgDn scroll | Esc close"
    };
    buf.set_stringn(
        inner.x,
        inner.bottom().saturating_sub(1),
        footer,
        usize::from(inner.width),
        Style::default().fg(theme.muted),
    );
}

fn paint_list(overlay: &mut PluginListOverlay, buf: &mut Buffer, theme: &Theme) {
    let visible = (usize::from(overlay.list_area.height) / 2).max(1);
    if overlay.selected < overlay.offset {
        overlay.offset = overlay.selected;
    } else if overlay.selected >= overlay.offset + visible {
        overlay.offset = overlay.selected + 1 - visible;
    }
    if overlay.plugins.is_empty() {
        let message = if overlay.loading {
            "Loading plugin inventory"
        } else if overlay.error.is_some() {
            "Plugin inventory unavailable"
        } else {
            "No plugins installed"
        };
        buf.set_stringn(
            overlay.list_area.x,
            overlay.list_area.y,
            message,
            usize::from(overlay.list_area.width),
            Style::default().fg(theme.muted),
        );
        return;
    }
    for index in overlay.offset..(overlay.offset + visible).min(overlay.plugins.len()) {
        let plugin = &overlay.plugins[index];
        let row = u16::try_from((index - overlay.offset) * 2).unwrap_or(0);
        let selected = index == overlay.selected;
        let state = if !plugin.enabled {
            "disabled"
        } else if plugin.loaded {
            "enabled / loaded"
        } else {
            "enabled / failed"
        };
        let label = format!("{} {}", if selected { ">" } else { " " }, plugin.name);
        let state_line = format!("  {state}");
        let style = if selected {
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.foreground)
        };
        buf.set_stringn(
            overlay.list_area.x,
            overlay.list_area.y + row,
            label,
            usize::from(overlay.list_area.width),
            style,
        );
        if row + 1 < overlay.list_area.height {
            let state_style = if plugin.loaded && plugin.enabled {
                Style::default().fg(theme.success)
            } else if plugin.enabled {
                Style::default().fg(theme.destructive)
            } else {
                Style::default().fg(theme.muted)
            };
            buf.set_stringn(
                overlay.list_area.x,
                overlay.list_area.y + row + 1,
                state_line,
                usize::from(overlay.list_area.width),
                state_style,
            );
        }
    }
}
