//! Output formatting: the event renderer, the debug views and small helpers.
//!
//! The renderer owns the streams; everything else here is a focused piece it
//! and the reports share. The submodules read their shared types through this
//! module, so the split never changed a call site.

mod argument;
mod debug;
mod dial;
mod edit;
mod helpers;
mod labels;
mod loading;
mod names;
mod renderer;
#[cfg(test)]
mod tests;

pub(crate) use argument::approval_label;
pub(crate) use debug::reasoning_setting_label;
pub(crate) use debug::{
    HostFacts, report_mcp, report_prompt, report_reasoning, report_start, report_tool_rules,
    report_tools,
};
pub(crate) use dial::{effort_span, reasoning_label, source_label, token_ratio};
pub(crate) use helpers::{cache_percent_label, fallback_env, find_project_config, human_tokens};
#[cfg(test)]
pub(crate) use labels::budget_label;
pub(crate) use labels::{
    format_measure, parse_budget_action, parse_budget_unit, parse_reasoning_setting, scope_name,
    select_system_prompt, short, unit_name,
};
pub(crate) use loading::Wait;
pub(crate) use renderer::{BudgetPrompt, Format, Renderer, Verbosity};

// The shared names the submodules read through `super::`.

use super::{
    ApprovalMode, BTreeMap, Budget, BudgetAction, BudgetScope, BudgetUnit, CacheMode, Config,
    Delta, DiffStyle, ErrorCode, Feature, Modality, ModelSpec, Path, Phase, Reasoning,
    ReasoningEffort, ReasoningInfo, RuleScope, ServerEvent, ServerEventKind, Service, SessionKey,
    Source, StreamingGranularity, StructuredOutputMode, Style, ToolKind, ToolRule, clip_text,
    compact_json, duration_label,
};
