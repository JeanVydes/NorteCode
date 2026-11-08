use dioxus::prelude::*;

pub fn stylesheet() -> Element {
    rsx! {
        document::Stylesheet { href: asset!("/assets/app.css") }
    }
}
