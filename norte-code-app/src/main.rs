//! Dioxus user interface for Norte Code's RAEP server.

#![forbid(unsafe_code)]

mod api;
mod app;
mod components;
mod domain;
mod pages;
mod state;
mod theme;
mod wire;

fn main() {
    dioxus::launch(app::App);
}
