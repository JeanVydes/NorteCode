//! Dioxus user interface for Norte Code's RAEP server.

#![forbid(unsafe_code)]

mod api;
mod app;
mod chat_client;
mod components;
mod domain;
mod frames;
mod pages;
mod sse;
mod state;
mod theme;
mod wire;

fn main() {
    dioxus::launch(app::App);
}
