//! Terminal interface for Norte Code.

#![forbid(unsafe_code)]

mod app;
mod chips;
mod client;
mod components;
mod config;
mod keys;
mod signals;
mod terminal;
mod text_builder;
mod tools;
mod ui_model;

fn main() -> Result<(), app::AppError> {
    app::run()
}
