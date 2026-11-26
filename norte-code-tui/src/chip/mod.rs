//! Composer chips: small labels standing for larger content.

mod content;
mod draft;
mod label;
mod unit;

pub(crate) use content::OriginalText;
pub(crate) use draft::Draft;
pub(crate) use label::ChipLabel;
pub(crate) use unit::{ChipId, ChipKind};

#[cfg(test)]
#[path = "chip_tests.rs"]
mod tests;
