use crate::components::{EmptyState, PageHeader};
use crate::domain::Capability;
use crate::state::AppContext;
use dioxus::prelude::*;

#[component]
pub fn SubagentsPage() -> Element {
    let context = use_context::<AppContext>();
    let supported = context
        .server_info
        .read()
        .as_ref()
        .is_some_and(|info| info.capabilities.contains(&Capability::Delegation));
    rsx! {
        PageHeader {
            eyebrow: "Delegation",
            title: "Subagents",
            description: "Inspect child sessions only when the server advertises and exposes the delegation profile."
        }
        if supported {
            EmptyState {
                title: "Inspector API is not connected",
                message: "The server advertises delegation, but this app build does not yet have a typed list/inspect endpoint. No child state is fabricated."
            }
        } else {
            EmptyState {
                title: "Delegation is unavailable",
                message: "The selected host does not advertise delegation, so child-session inspection and controls are disabled."
            }
        }
    }
}
