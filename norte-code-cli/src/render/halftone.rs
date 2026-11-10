//! Reusable halftone chart primitives for the terminal UI.

#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use ratatui::{
    Frame,
    buffer::Buffer,
    layout::{Margin, Rect},
    style::{Color, Style as RatatuiStyle},
    widgets::{Block, Borders, Widget},
};

/// Character scale used by halftone marks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum DotSize {
    /// Light, fine-grained marks.
    Small,
    /// Balanced marks.
    #[default]
    Medium,
    /// Heavy marks.
    Large,
}

/// Shared visual configuration for all halftone charts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct HalftoneConfig {
    /// Contrast multiplier, normalized to `0.0..=1.0`.
    pub(crate) contrast: f32,
    /// Dot coverage, normalized to `0.0..=1.0`.
    pub(crate) density: f32,
    /// Character scale.
    pub(crate) dot_size: DotSize,
}

impl Default for HalftoneConfig {
    fn default() -> Self {
        Self {
            contrast: 0.75,
            density: 0.82,
            dot_size: DotSize::Medium,
        }
    }
}

impl HalftoneConfig {
    /// Converts the persisted UI settings into bounded rendering values.
    #[must_use]
    pub(crate) fn from_chart_config(config: &norte_code_protocol::config::ChartConfig) -> Self {
        let dot_size = match config.dot_size {
            norte_code_protocol::config::ChartDotSize::Small => DotSize::Small,
            norte_code_protocol::config::ChartDotSize::Medium => DotSize::Medium,
            norte_code_protocol::config::ChartDotSize::Large => DotSize::Large,
        };
        Self {
            contrast: f32::from(config.contrast.min(100)) / 100.0,
            density: f32::from(config.density.min(100)) / 100.0,
            dot_size,
        }
    }

    fn glyph(self, intensity: f32) -> char {
        let value = (intensity * self.contrast).clamp(0.0, 1.0);
        match self.dot_size {
            DotSize::Small => {
                if value > 0.62 {
                    '•'
                } else {
                    '·'
                }
            }
            DotSize::Medium => {
                if value > 0.76 {
                    '●'
                } else if value > 0.42 {
                    '•'
                } else {
                    '·'
                }
            }
            DotSize::Large => {
                if value > 0.68 {
                    '●'
                } else {
                    '•'
                }
            }
        }
    }

    fn accepts(self, x: usize, y: usize, intensity: f32) -> bool {
        if intensity <= 0.0 {
            return false;
        }
        let hash = ((x * 37 + y * 17 + x * y * 3) % 100) as f32 / 100.0;
        hash <= (self.density * intensity).clamp(0.0, 1.0)
    }
}

/// A line series consumed by the reusable chart renderer.
pub(crate) struct Series<'a> {
    /// `(x, y)` points.
    pub(crate) points: &'a [(f64, f64)],
    /// Series colour.
    pub(crate) color: Color,
}

/// Draws a halftone line chart.
pub(crate) fn line(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &str,
    series: &[Series<'_>],
    config: HalftoneConfig,
) {
    let inner = chart_inner(frame, area, title);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let max_x = series
        .iter()
        .flat_map(|item| item.points.iter().map(|(x, _)| *x))
        .fold(1.0, f64::max);
    let max_y = series
        .iter()
        .flat_map(|item| item.points.iter().map(|(_, y)| *y))
        .fold(1.0, f64::max);
    for item in series {
        for pair in item.points.windows(2) {
            let [(x0, y0), (x1, y1)] = pair else {
                continue;
            };
            let steps = (*x1 - *x0).abs().max(1.0) as usize * 3;
            for step in 0..=steps {
                let t = step as f64 / steps as f64;
                let x = x0 + (x1 - x0) * t;
                let y = y0 + (y1 - y0) * t;
                let px = ((x / max_x) * f64::from(inner.width.saturating_sub(1))) as usize;
                let py = ((1.0 - y / max_y) * f64::from(inner.height.saturating_sub(1))) as usize;
                let intensity = 0.72 + 0.28 * (1.0 - t.min(1.0) as f32);
                put_dot(
                    frame.buffer_mut(),
                    inner,
                    px,
                    py,
                    intensity,
                    item.color,
                    config,
                );
            }
        }
    }
}

/// Draws a halftone pie chart with a legend.
pub(crate) fn pie(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &str,
    slices: &[(&str, f64, Color)],
    config: HalftoneConfig,
) {
    let inner = chart_inner(frame, area, title);
    if inner.width == 0 || inner.height == 0 || slices.is_empty() {
        return;
    }
    let total = slices
        .iter()
        .map(|(_, value, _)| value.max(0.0))
        .sum::<f64>();
    if total <= f64::EPSILON {
        return;
    }
    let radius = f64::from(inner.height.min(inner.width / 2).saturating_sub(1));
    let cx = f64::from(inner.x) + f64::from(inner.width.min(34)) * 0.42;
    let cy = f64::from(inner.y) + f64::from(inner.height) * 0.5;
    for y in 0..inner.height {
        for x in 0..inner.width.min(34) {
            let dx = (f64::from(inner.x + x) - cx) / 2.0;
            let dy = f64::from(inner.y + y) - cy;
            let distance = (dx * dx + dy * dy).sqrt();
            if distance > radius {
                continue;
            }
            let mut angle = dy.atan2(dx) / std::f64::consts::TAU;
            angle = angle.rem_euclid(1.0);
            let mut cursor = 0.0;
            for (name, value, color) in slices {
                let share = value.max(0.0) / total;
                if angle < cursor + share {
                    let edge = (angle - cursor).min(cursor + share - angle) * 30.0;
                    let intensity = if edge < 0.8 {
                        0.45
                    } else {
                        1.0 - distance / radius * 0.35
                    };
                    put_dot(
                        frame.buffer_mut(),
                        inner,
                        x as usize,
                        y as usize,
                        intensity as f32,
                        *color,
                        config,
                    );
                    let _ = name;
                    break;
                }
                cursor += share;
            }
        }
    }
    let legend_x = inner.x + 36.min(inner.width.saturating_sub(1));
    for (index, (name, value, color)) in slices.iter().enumerate() {
        let y = inner.y + u16::try_from(index).unwrap_or(u16::MAX);
        if y >= inner.y + inner.height {
            break;
        }
        set(frame.buffer_mut(), legend_x, y, '■', *color);
        for (offset, ch) in name.chars().enumerate() {
            let x = legend_x
                .saturating_add(2)
                .saturating_add(u16::try_from(offset).unwrap_or(u16::MAX));
            if x < inner.x + inner.width {
                set(frame.buffer_mut(), x, y, ch, Color::Reset);
            }
        }
        let label = format!("{:.0}%", value / total * 100.0);
        for (offset, ch) in label.chars().enumerate() {
            let x = legend_x + inner.width.saturating_sub(5) + u16::try_from(offset).unwrap_or(0);
            if x < inner.x + inner.width {
                set(frame.buffer_mut(), x, y, ch, *color);
            }
        }
    }
}

/// Draws a two-profile radar chart using halftone fills.
pub(crate) fn radar(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &str,
    profiles: &[(&str, &[f64], Color)],
    config: HalftoneConfig,
) {
    let inner = chart_inner(frame, area, title);
    if inner.width == 0 || inner.height == 0 || profiles.is_empty() {
        return;
    }
    let axes = profiles
        .iter()
        .map(|(_, values, _)| values.len())
        .min()
        .unwrap_or(0);
    if axes < 3 {
        return;
    }
    let cx = f64::from(inner.x + inner.width.min(38) / 2);
    let cy = f64::from(inner.y + inner.height / 2);
    let radius = f64::from(inner.height.min(inner.width.min(38) / 2).saturating_sub(1));
    let polygons = profiles
        .iter()
        .map(|(_, values, _)| {
            (0..axes)
                .map(|index| {
                    let angle = -std::f64::consts::FRAC_PI_2
                        + index as f64 / axes as f64 * std::f64::consts::TAU;
                    (
                        cx + angle.cos() * values[index].clamp(0.0, 1.0) * radius * 2.0,
                        cy + angle.sin() * values[index].clamp(0.0, 1.0) * radius,
                    )
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    for y in 0..inner.height {
        for x in 0..inner.width.min(38) {
            let px = f64::from(inner.x + x);
            let py = f64::from(inner.y + y);
            for (index, polygon) in polygons.iter().enumerate().rev() {
                if inside(px, py, polygon) {
                    let color = profiles[index].2;
                    put_dot(
                        frame.buffer_mut(),
                        inner,
                        x as usize,
                        y as usize,
                        0.9,
                        color,
                        config,
                    );
                    break;
                }
            }
        }
    }
    let legend_x = inner.x + 40.min(inner.width.saturating_sub(1));
    for (index, (name, _, color)) in profiles.iter().enumerate() {
        let y = inner.y + u16::try_from(index).unwrap_or(u16::MAX);
        if y < inner.y + inner.height {
            set(frame.buffer_mut(), legend_x, y, '■', *color);
            for (offset, ch) in name.chars().enumerate() {
                let x = legend_x
                    .saturating_add(2)
                    .saturating_add(u16::try_from(offset).unwrap_or(u16::MAX));
                if x < inner.x + inner.width {
                    set(frame.buffer_mut(), x, y, ch, Color::Reset);
                }
            }
        }
    }
}

fn chart_inner(frame: &mut Frame<'_>, area: Rect, title: &str) -> Rect {
    Block::default()
        .borders(Borders::ALL)
        .title(title)
        .render(area, frame.buffer_mut());
    area.inner(Margin {
        vertical: 1,
        horizontal: 1,
    })
}

fn put_dot(
    buffer: &mut Buffer,
    area: Rect,
    x: usize,
    y: usize,
    intensity: f32,
    color: Color,
    config: HalftoneConfig,
) {
    if x >= usize::from(area.width)
        || y >= usize::from(area.height)
        || !config.accepts(x, y, intensity)
    {
        return;
    }
    set(
        buffer,
        area.x + x as u16,
        area.y + y as u16,
        config.glyph(intensity),
        color,
    );
}

fn set(buffer: &mut Buffer, x: u16, y: u16, ch: char, color: Color) {
    if let Some(cell) = buffer.cell_mut((x, y)) {
        cell.set_char(ch)
            .set_style(RatatuiStyle::default().fg(color));
    }
}

fn inside(x: f64, y: f64, polygon: &[(f64, f64)]) -> bool {
    let mut inside = false;
    let mut previous = polygon.len().saturating_sub(1);
    for current in 0..polygon.len() {
        let (x0, y0) = polygon[current];
        let (x1, y1) = polygon[previous];
        if (y0 > y) != (y1 > y) && x < (x1 - x0) * (y - y0) / (y1 - y0) + x0 {
            inside = !inside;
        }
        previous = current;
    }
    inside
}

#[cfg(test)]
#[path = "halftone_tests.rs"]
mod tests;
