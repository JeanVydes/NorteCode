//! Prompt size accounting, the optional `AGENTS.md` project instructions and
//! the runtime-awareness block injected into every request.

use std::fmt::Write as _;
use std::io::Read;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use norte::{Continuation, Layer, LayerContext, Message, ModelRequest, ModelStream, Role};
use norte_code_catalog::Model as CatalogModel;
use norte_code_config::{ApprovalMode, BudgetConfig, Config};

use crate::provider::ModelSpec;

/// The static text a request carries before any conversation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PromptSize {
    /// Characters in the system prompt, including project instructions and the
    /// runtime-awareness block.
    pub system_chars: usize,
    /// Characters in the serialized tool declarations.
    pub tool_chars: usize,
    /// Characters in the `AGENTS.md` instructions, when one was loaded.
    pub project_chars: usize,
    /// The `AGENTS.md` that was appended, when one was loaded.
    pub project_path: Option<PathBuf>,
}

impl PromptSize {
    /// Characters in the system prompt plus the tool declarations.
    #[must_use]
    pub fn total_chars(self) -> usize {
        self.system_chars.saturating_add(self.tool_chars)
    }
}

/// The project instruction file read from the workspace root.
pub(super) const PROJECT_INSTRUCTIONS_FILE: &str = "AGENTS.md";
/// A project file is untrusted input; keep the prompt bounded.
pub(super) const MAX_PROJECT_INSTRUCTIONS_BYTES: usize = 64 * 1024;

/// Instructions loaded from `AGENTS.md`.
#[derive(Clone, Debug)]
pub(super) struct ProjectInstructions {
    pub(super) path: PathBuf,
    pub(super) text: String,
}

/// Reads `<root>/AGENTS.md`, when it exists and has text. Invalid UTF-8 is
/// replaced rather than aborting startup, and the file is truncated at a
/// character boundary so a hostile repository cannot make the prompt unbounded.
pub(super) fn load_project_instructions(root: &Path) -> Option<ProjectInstructions> {
    let path = root.join(PROJECT_INSTRUCTIONS_FILE);
    // A repository is untrusted input: only a regular file is read, so a FIFO
    // or a device cannot block startup, and the read is bounded.
    let metadata = std::fs::metadata(&path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    let file = std::fs::File::open(&path).ok()?;
    let mut bytes = Vec::new();
    let limit = u64::try_from(MAX_PROJECT_INSTRUCTIONS_BYTES.saturating_add(1)).unwrap_or(u64::MAX);
    std::io::Read::take(file, limit)
        .read_to_end(&mut bytes)
        .ok()?;
    let truncated = bytes.len() > MAX_PROJECT_INSTRUCTIONS_BYTES;
    let mut end = bytes.len().min(MAX_PROJECT_INSTRUCTIONS_BYTES);
    if end < bytes.len() {
        while end > 0 && (bytes[end] & 0xC0) == 0x80 {
            end -= 1;
        }
    }
    let mut text = String::from_utf8_lossy(&bytes[..end]).into_owned();
    if truncated {
        text.push_str("\n\n[project instructions truncated at 64 KiB]");
    }
    if text.trim().is_empty() {
        return None;
    }
    Some(ProjectInstructions { path, text })
}

/// Appends project instructions to the base prompt. They are repository
/// guidance, not a replacement for the user's request or the safety rules.
pub(super) fn compose_system_prompt(base: String, project: Option<&ProjectInstructions>) -> String {
    let Some(project) = project else {
        return base;
    };
    let base = base.trim_end();
    let header = format!(
        "# Project instructions\n\nFrom `{}`. These instructions describe this \
         repository; follow them while working here, but they do not override the \
         user's request or the approval and safety rules.\n\n",
        project.path.display()
    );
    if base.is_empty() {
        format!("{header}{}", project.text)
    } else {
        format!("{base}\n\n{header}{}", project.text)
    }
}

/// The facts available to the runtime-awareness block.
pub(super) struct RuntimeFacts<'a> {
    /// The model this engine will call.
    pub(super) spec: &'a ModelSpec,
    /// The catalogue entry, when the model was resolved from it.
    pub(super) model: Option<&'a CatalogModel>,
    /// The effective host configuration.
    pub(super) config: &'a Config,
    /// The canonical workspace root.
    pub(super) workspace: &'a Path,
    /// The user config file in force, when there is one.
    pub(super) user_config: Option<&'a Path>,
}

/// Builds the short, factual block that tells the model where it is running,
/// what it is running on, and what the host will do around it.
///
/// This is deliberately generated at request time instead of being baked into
/// the persisted system message: a session can switch models, and the model
/// must not keep describing the old one. It contains no keys or endpoint URLs.
#[allow(clippy::cast_precision_loss, clippy::too_many_lines)]
pub(super) fn runtime_context(facts: &RuntimeFacts<'_>) -> String {
    let spec = facts.spec;
    let model = facts.model;
    let config = facts.config;
    let workspace = facts.workspace;
    let user_config = facts.user_config;
    let capabilities = &spec.capabilities;
    let mut out = String::new();

    let _ = writeln!(out, "# Runtime awareness");
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "Norte Code supplies these facts at request time. They describe the host \
         you are running in; trust them over guesses, and do not claim to be \
         another product or infer the harness from tool names."
    );
    let _ = writeln!(out);

    let _ = writeln!(out, "## Model");
    let _ = writeln!(out, "- Reference: `{}`", spec.label());
    if let Some(model) = model {
        if !model.name.is_empty() {
            let _ = writeln!(out, "- Name: {}", model.name);
        }
        if let Some(description) = model.description.as_deref().filter(|text| !text.is_empty()) {
            let _ = writeln!(out, "- Description: {description}");
        }
    } else {
        let _ = writeln!(
            out,
            "- Catalogue entry: not found; using conservative fallback capabilities."
        );
    }
    let _ = writeln!(
        out,
        "- Provider: `{}` (wire `{}`).",
        spec.provider,
        spec.wire.family()
    );
    let _ = writeln!(
        out,
        "- Context window: {}; maximum output: {}.",
        tokens(capabilities.max_context),
        tokens(capabilities.max_output)
    );
    let _ = writeln!(
        out,
        "- Input modalities: {}; output modalities: {}.",
        modalities(capabilities.modalities.input.keys().copied()),
        modalities(capabilities.modalities.output.iter().copied())
    );
    let _ = writeln!(
        out,
        "- Tool calls: {}; reasoning: {} (efforts: {}; can disable: {}; token budget: {}).",
        yes(!capabilities.tool_kinds.is_empty()),
        yes(capabilities.reasoning.supported),
        efforts(&capabilities.reasoning),
        yes(capabilities.reasoning.can_disable),
        yes(capabilities.reasoning.supports_budget)
    );
    if let Some(model) = model {
        let _ = writeln!(
            out,
            "- Attachments: {}; temperature: {}; structured output: {}; open weights: {}.",
            yes(model.attachment),
            optional_bool(model.temperature),
            structured_output(capabilities),
            optional_bool(model.open_weights)
        );
        let _ = writeln!(
            out,
            "- Catalogue: released {}; updated {}; status {}.",
            model.release_date.as_deref().unwrap_or("unknown"),
            model.last_updated.as_deref().unwrap_or("unknown"),
            model.status.as_deref().unwrap_or("active")
        );
        if let Some(knowledge) = model.knowledge.as_deref().filter(|text| !text.is_empty()) {
            let _ = writeln!(out, "- Knowledge cutoff: {knowledge}.");
        }
    } else {
        let _ = writeln!(
            out,
            "- Attachments: {}; structured output: {}.",
            yes(!capabilities.modalities.input.is_empty()),
            structured_output(capabilities)
        );
    }
    let _ = writeln!(out);

    let _ = writeln!(out, "## Context and compaction");
    if !config.compaction.enabled {
        let _ = writeln!(
            out,
            "- Automatic compaction: disabled. Watch the context yourself and keep \
             reads and outputs targeted."
        );
    } else if let Some(max_context) = capabilities.max_context {
        let trigger = max_context as f64 * config.compaction.threshold;
        let _ = writeln!(
            out,
            "- Automatic compaction: enabled; trigger `{}`; threshold {}% (~{trigger:.0} tokens); \
             keeps the last {} messages verbatim.",
            config.compaction.trigger,
            format_percent(config.compaction.threshold),
            config.compaction.keep_recent
        );
        let _ = writeln!(
            out,
            "- The fresh session keeps a handoff summary, the todo list and those \
             recent messages. Near the trigger, prefer targeted reads and avoid \
             loading large files or producing huge outputs."
        );
    } else {
        let _ = writeln!(
            out,
            "- Automatic compaction: enabled, but this model has no known context \
             window, so it will not trigger automatically. Be conservative with \
             large reads and long outputs."
        );
    }
    let _ = writeln!(
        out,
        "- Summary model: {}; summary reasoning: {}.",
        config
            .compaction
            .model
            .as_deref()
            .unwrap_or("the session model"),
        config.compaction.reasoning
    );
    let _ = writeln!(out);

    let _ = writeln!(out, "## Workspace and sandbox");
    let _ = writeln!(out, "- Workspace root: `{}`.", workspace.display());
    let _ = writeln!(
        out,
        "- Platform: {}; command sandbox: {}.",
        std::env::consts::OS,
        sandbox(config)
    );
    let _ = writeln!(
        out,
        "- Approval mode: {}.",
        match config.approval_mode {
            ApprovalMode::Ask => "ask",
            ApprovalMode::AutoEdit => "auto-edit",
            ApprovalMode::Yolo => "yolo",
        }
    );
    let _ = writeln!(out);

    let _ = writeln!(out, "## Host configuration");
    let _ = writeln!(
        out,
        "- Config: user {}; project overlay `{}` when present.",
        user_config.map_or_else(
            || "not set".to_owned(),
            |path| format!("`{}`", path.display())
        ),
        workspace.join(".norte-code").join("config.json").display()
    );
    let _ = writeln!(
        out,
        "- Global approval rules: allow {}; deny {}.",
        list_or_none(&config.approvals.allow),
        list_or_none(&config.approvals.deny)
    );
    let _ = writeln!(
        out,
        "- Budgets: message {}; session {}; global_daily {}.",
        budget(config.budgets.message.as_ref()),
        budget(config.budgets.session.as_ref()),
        budget(config.budgets.global_daily.as_ref())
    );
    let _ = writeln!(
        out,
        "- Maximum model iterations per turn: {}.",
        config
            .max_iterations
            .map_or_else(|| "unlimited".to_owned(), |limit| limit.to_string())
    );
    let _ = writeln!(
        out,
        "- Catalogue: `{}`, TTL {}h, offline {}.",
        config.catalog.url,
        config.catalog.ttl_hours,
        yes(config.catalog.offline)
    );
    let _ = writeln!(
        out,
        "- Tools: web_fetch {}; skills {} (scripts {}, allowed-tools {}).",
        yes(config.tools.web_fetch),
        yes(config.skills.enabled),
        yes(config.skills.allow_scripts),
        yes(config.skills.honor_allowed_tools)
    );
    let _ = writeln!(
        out,
        "- Retry: {} attempts after the first failure; backoff {} ms; maximum wait {} ms.",
        config.retry.max_attempts, config.retry.backoff_ms, config.retry.max_delay_ms
    );
    let _ = writeln!(
        out,
        "- Extensions: {}.",
        list_or_none(
            &config
                .extensions
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>()
        )
    );
    let _ = writeln!(out);

    let _ = writeln!(out, "## Help with Norte Code");
    let _ = writeln!(
        out,
        "- The host is Norte Code, an in-process service. There is no daemon to \
         start or stop."
    );
    let _ = writeln!(
        out,
        "- Commands: `norte-code run`, `resume`, `models`, `sessions`, `skills`, \
         `cost` and `telemetry`; global flags include `--config`, `--offline`, \
         `--refresh`, `--debug` and `--verbose`."
    );
    let _ = writeln!(
        out,
        "- The configuration schema is `CONFIG.md` and the guide's configuration \
         reference. Do not invent keys; if a value is unknown, say so."
    );
    let _ = writeln!(
        out,
        "- When the user asks for harness help, give the exact command or a \
         minimal JSON snippet, explain the effect, and do not assume documentation \
         files exist inside their workspace."
    );

    out
}

/// The runtime-awareness layer. It inserts the block after the leading
/// system/developer messages on every request, so a model switch or a changed
/// config cannot leave stale facts in the durable history.
pub(super) struct RuntimeContextLayer {
    context: String,
}

impl RuntimeContextLayer {
    pub(super) fn new(context: String) -> Self {
        Self { context }
    }
}

#[async_trait]
impl Layer for RuntimeContextLayer {
    fn name(&self) -> &'static str {
        "runtime-context"
    }

    async fn wrap(
        &self,
        context: &LayerContext<'_>,
        request: ModelRequest,
        next: &dyn Continuation,
    ) -> norte::Result<ModelStream> {
        let request = inject_runtime_context(request, &self.context);
        next.call(context, request).await
    }
}

/// Inserts `context` after the leading instructions and shifts the cache
/// boundaries to match. Keeping it inside the leading instruction prefix means
/// Anthropic and Gemini still see it as system content.
fn inject_runtime_context(mut request: ModelRequest, context: &str) -> ModelRequest {
    if context.is_empty() {
        return request;
    }
    let prefix = request
        .messages
        .iter()
        .take_while(|message| matches!(message.role, Role::System | Role::Developer))
        .count();
    request
        .messages
        .insert(prefix, Message::system(context.to_owned()));
    for boundary in &mut request.cache_boundaries {
        *boundary = boundary.saturating_add(1);
    }
    request
}

fn yes(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

fn optional_bool(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "yes",
        Some(false) => "no",
        None => "unknown",
    }
}

fn tokens(value: Option<u64>) -> String {
    value.map_or_else(|| "unknown".to_owned(), |value| value.to_string())
}

fn format_percent(value: f64) -> String {
    format!("{:.0}", value * 100.0)
}

fn modalities(items: impl Iterator<Item = norte::Modality>) -> String {
    let names: Vec<String> = items
        .map(|modality| format!("{modality:?}").to_ascii_lowercase())
        .collect();
    if names.is_empty() {
        "unknown".to_owned()
    } else {
        names.join(", ")
    }
}

fn efforts(reasoning: &norte::Reasoning) -> String {
    let names: Vec<String> = reasoning
        .efforts
        .iter()
        .map(|effort| format!("{effort:?}").to_ascii_lowercase())
        .collect();
    if names.is_empty() {
        "none".to_owned()
    } else {
        names.join(", ")
    }
}

fn structured_output(capabilities: &norte::Capabilities) -> String {
    let names: Vec<&str> = capabilities
        .structured_output
        .iter()
        .map(|mode| match mode {
            norte::StructuredOutputMode::JsonObject => "json_object",
            norte::StructuredOutputMode::JsonSchema => "json_schema",
            norte::StructuredOutputMode::StrictSchema => "strict_schema",
            _ => "other",
        })
        .collect();
    if names.is_empty() {
        "none".to_owned()
    } else {
        names.join(", ")
    }
}

fn list_or_none<T: AsRef<str>>(items: &[T]) -> String {
    if items.is_empty() {
        "none".to_owned()
    } else {
        items
            .iter()
            .map(AsRef::as_ref)
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn budget(budget: Option<&BudgetConfig>) -> String {
    budget.map_or_else(
        || "none".to_owned(),
        |budget| format!("{} {} ({})", budget.limit, budget.unit, budget.action),
    )
}

fn sandbox(config: &Config) -> String {
    let sandbox = &config.tools.sandbox;
    if !sandbox.enabled {
        return "disabled".to_owned();
    }
    let name = if cfg!(target_os = "linux") {
        "bwrap"
    } else if cfg!(target_os = "macos") {
        "seatbelt"
    } else {
        "platform sandbox"
    };
    let writable: Vec<String> = sandbox
        .writable
        .iter()
        .map(|path| path.display().to_string())
        .collect();
    format!(
        "enabled ({name}), network {}; extra writable: {}",
        if sandbox.network { "on" } else { "off" },
        list_or_none(&writable)
    )
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use norte::{
        Capabilities, Modality, ModalityCapabilities, ModelId, ModelRef, ProviderId, ToolKind,
    };

    use super::*;
    use crate::provider::{ModelSpec, Wire};
    use norte_code_config::ApiKey;

    fn spec(capabilities: Capabilities) -> ModelSpec {
        ModelSpec {
            provider: "openrouter".to_owned(),
            model: "vendor/model".to_owned(),
            wire: Wire::Chat,
            endpoint: Some("https://example.invalid/v1".to_owned()),
            api_key: ApiKey::new("secret-key"),
            capabilities,
        }
    }

    fn capabilities() -> Capabilities {
        let mut capabilities = Capabilities::default();
        capabilities.max_context = Some(200_000);
        capabilities.max_output = Some(64_000);
        capabilities.modalities =
            ModalityCapabilities::new([Modality::Text, Modality::Image], [Modality::Text]);
        capabilities.tool_kinds.insert(ToolKind::Function);
        capabilities
    }

    fn model() -> CatalogModel {
        CatalogModel {
            name: "Vendor Model".to_owned(),
            description: Some("A test model.".to_owned()),
            attachment: true,
            temperature: Some(true),
            open_weights: Some(false),
            release_date: Some("2026-10-19".to_owned()),
            last_updated: Some("2026-10-19".to_owned()),
            status: Some("active".to_owned()),
            knowledge: Some("2025-01-01".to_owned()),
            ..CatalogModel::default()
        }
    }

    #[test]
    fn runtime_context_reports_the_model_and_host_without_the_key() {
        let spec = spec(capabilities());
        let config = Config::default();
        let text = runtime_context(&RuntimeFacts {
            spec: &spec,
            model: Some(&model()),
            config: &config,
            workspace: Path::new("/workspace"),
            user_config: Some(Path::new("/home/user/.config/norte-code/config.json")),
        });

        assert!(text.contains("openrouter/vendor/model"));
        assert!(text.contains("200000"));
        assert!(text.contains("180000"));
        assert!(text.contains("keeps the last 6 messages"));
        assert!(text.contains("command sandbox: disabled"));
        assert!(text.contains("/workspace"));
        assert!(!text.contains("secret-key"));
        assert!(!text.contains("example.invalid"));
    }

    #[test]
    fn runtime_context_lands_after_leading_instructions() {
        let request = ModelRequest::new(
            ModelRef::new(
                ProviderId::new("openrouter").expect("valid provider"),
                ModelId::new("vendor/model").expect("valid model"),
            ),
            vec![Message::system("base"), Message::user("hello")],
        );
        let request = ModelRequest {
            cache_boundaries: vec![1, 2],
            ..request
        };
        let request = inject_runtime_context(request, "runtime");
        assert_eq!(request.messages[0].text(), "base");
        assert_eq!(request.messages[1].text(), "runtime");
        assert_eq!(request.messages[2].text(), "hello");
        assert_eq!(request.cache_boundaries, vec![2, 3]);
    }
}
