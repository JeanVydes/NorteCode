//! Shared chrome construction for bordered panels.

use ratatui::style::Color;
use terminalui::components::chrome::{Border, BorderSet, Chrome, Padding, Radius};

/// A rounded panel whose border, background and padding vary by caller.
pub(crate) fn rounded(border_color: Color, background: Option<Color>, padding: Padding) -> Chrome {
    Chrome {
        border: Some(Border {
            set: BorderSet::Rounded,
            color: Some(border_color),
            radius: Radius::Small,
        }),
        background,
        radius: Radius::Small,
        padding,
        border_gradient: None,
        fill_gradient: None,
    }
}
