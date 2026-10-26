//! The `--debug` views: what is running, what it can do and where it comes
//! from, laid out as aligned panels.

use norte_code_catalog::{Catalog, Model};

use super::helpers::find_project_config;
use super::names::{
    cache_name, feature_set, modality_set, reasoning_capabilities, streaming_name, structured_set,
    tool_kind_set, yes_no,
};
use super::{Config, ModelSpec, Path, Reasoning, Service, Source, Style};
use super::{approval_label, source_label};

/// One line of a debug panel: either a row or a rule.
enum DebugRow {
    /// A field or a heading, laid out in the panel's columns.
    Row { left: String, right: String },
    /// A horizontal rule that spans both columns.
    Rule,
}

/// Collects rows and prints them as one aligned panel.
///
/// The columns are sized from the rows, so a long path never breaks the frame
/// and every panel lines up whatever it holds. The frame is drawn only when the
/// destination takes escapes: cloning it for a pipe or a log file would be
/// noise, so those get the plain rows instead.
struct DebugPanel {
    left_title: &'static str,
    right_title: &'static str,
    rows: Vec<DebugRow>,
}

impl DebugPanel {
    fn new(left_title: &'static str, right_title: &'static str) -> Self {
        Self {
            left_title,
            right_title,
            rows: Vec::new(),
        }
    }

    /// Adds a `left` to `right` row.
    fn field(&mut self, left: impl Into<String>, right: impl Into<String>) -> &mut Self {
        self.rows.push(DebugRow::Row {
            left: left.into(),
            right: right.into(),
        });
        self
    }

    /// Adds a rule, so two kinds of fact can be told apart.
    fn rule(&mut self) -> &mut Self {
        self.rows.push(DebugRow::Rule);
        self
    }

    /// The panel as lines, one per row, ready to print or to keep.
    ///
    /// Returning them rather than writing them is what lets the same view be
    /// printed at startup or shown as a tab.
    fn render(&self, style: Style) -> Vec<String> {
        let mut lines = Vec::new();
        let left_width = self
            .rows
            .iter()
            .filter_map(|row| match row {
                DebugRow::Row { left, .. } => Some(display_width(left)),
                DebugRow::Rule => None,
            })
            .chain(std::iter::once(display_width(self.left_title)))
            .max()
            .unwrap_or(0);
        let right_width = self
            .rows
            .iter()
            .filter_map(|row| match row {
                DebugRow::Row { right, .. } => Some(display_width(right)),
                DebugRow::Rule => None,
            })
            .chain(std::iter::once(display_width(self.right_title)))
            .max()
            .unwrap_or(0);
        if !style.paints() {
            for row in &self.rows {
                match row {
                    DebugRow::Row { left, right } => {
                        lines.push(format!("  {} {right}", pad(left, left_width)));
                    }
                    DebugRow::Rule => {
                        lines.push(format!(
                            "  {}",
                            style.dim("─".repeat(left_width + right_width + 3))
                        ));
                    }
                }
            }
            return lines;
        }
        let top = format!(
            "┌{}┬{}┐",
            "─".repeat(left_width + 2),
            "─".repeat(right_width + 2)
        );
        let middle = format!(
            "├{}┼{}┤",
            "─".repeat(left_width + 2),
            "─".repeat(right_width + 2)
        );
        let bottom = format!(
            "└{}┴{}┘",
            "─".repeat(left_width + 2),
            "─".repeat(right_width + 2)
        );
        lines.push(format!("  {}", style.faint(top)));
        lines.push(format!(
            "  {} {} {} {} {}",
            style.faint("│"),
            style.bold(pad_title(self.left_title, left_width)),
            style.faint("│"),
            style.bold(pad_title(self.right_title, right_width)),
            style.faint("│")
        ));
        lines.push(format!("  {}", style.faint(middle.clone())));
        for row in &self.rows {
            match row {
                DebugRow::Row { left, right } => {
                    lines.push(format!(
                        "  {} {} {} {} {}",
                        style.faint("│"),
                        style.dim(pad(left, left_width)),
                        style.faint("│"),
                        pad(right, right_width),
                        style.faint("│")
                    ));
                }
                DebugRow::Rule => lines.push(format!("  {}", style.faint(middle.clone()))),
            }
        }
        lines.push(format!("  {}", style.faint(bottom)));
        lines
    }
}

/// How many columns a string occupies, ignoring escapes and refusing to split a
/// character: the panel aligns on what the terminal shows, not on bytes.
fn display_width(text: &str) -> usize {
    crate::text::terminal(text).chars().count()
}

/// Pads a value to the column width it was measured at.
fn pad(text: &str, width: usize) -> String {
    let text = crate::text::terminal(text);
    let shown = text.chars().count();
    format!("{text}{}", " ".repeat(width.saturating_sub(shown)))
}

/// Pads a panel title, which is trusted chrome and is not escaped.
fn pad_title(title: &str, width: usize) -> String {
    format!(
        "{title}{}",
        " ".repeat(width.saturating_sub(title.chars().count()))
    )
}

/// One `debug:` section title, so every block reads as a heading.
///
/// The title is escaped like any other text, then wrapped in the codes: the
/// escaping runs before the escapes are added, so the heading is not turned
/// into `\u{1b}[1m`.
fn debug_heading(style: Style, title: &str) -> String {
    let title = crate::text::terminal(title);
    // Both codes go on in one run: nesting `cyan(bold(..))` would escape the
    // inner codes and print them instead of styling the title.
    if style.paints() {
        style.paint_raw("1;36", title.as_ref())
    } else {
        title.into_owned()
    }
}

/// Prints the startup facts that make a run or resume reproducible.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(crate) fn debug_start(
    style: Style,
    spec: &ModelSpec,
    configuration: &Config,
    source: Source,
    database: &Path,
    config_path: Option<&Path>,
    session: Option<&str>,
    catalog: &Catalog,
) -> Vec<String> {
    let workspace =
        std::env::current_dir().map_or_else(|_| "?".to_owned(), |path| path.display().to_string());
    let config_label =
        config_path.map_or_else(|| "(default)".to_owned(), |path| path.display().to_string());
    let project_config = find_project_config(&std::env::current_dir().unwrap_or_default())
        .map_or_else(|| "(none)".to_owned(), |path| path.display().to_string());
    let sandbox = if configuration.tools.confinement.sandbox.enabled {
        let name = if cfg!(target_os = "linux") {
            "bwrap"
        } else if cfg!(target_os = "macos") {
            "seatbelt"
        } else {
            "unsupported platform"
        };
        format!(
            "enabled · {name} · network {}",
            if configuration.tools.confinement.sandbox.network {
                "on"
            } else {
                "off"
            }
        )
    } else {
        "disabled".to_owned()
    };
    // The threshold is a fraction between 0 and 1, so rounding it to whole
    // percent never drops a meaningful digit.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let compaction_pct = (configuration.compaction.threshold * 100.0).round() as u64;
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    let compaction_tokens = spec
        .capabilities
        .max_context
        .map(|context| (context as f64 * configuration.compaction.threshold).round() as u64);
    let provider_count = catalog.providers.len();
    let model_count: usize = catalog.providers.values().map(|p| p.models.len()).sum();
    let max_iterations = configuration
        .max_iterations
        .map_or_else(|| "unlimited".to_owned(), |limit| limit.to_string());
    let mut panel = DebugPanel::new("session", "value");
    panel
        .field("key", session.unwrap_or("(new; shown on first event)"))
        .field("model", spec.label())
        .field(
            "provider",
            format!(
                "{}{}",
                spec.provider,
                if catalog.provider(&spec.provider).is_some() {
                    String::new()
                } else {
                    "  (not in catalogue; using config/fallback)".to_owned()
                }
            ),
        )
        .field("wire", spec.wire.family())
        .field("approval", approval_label(configuration.approval_mode))
        .field("iterations", &max_iterations)
        .field(
            "compaction",
            format!(
                "{}%{}{}",
                compaction_pct,
                compaction_tokens
                    .map(|tokens| format!(" (~{tokens} tokens)"))
                    .unwrap_or_default(),
                if !configuration.compaction.enabled {
                    " (disabled)"
                } else if configuration.compaction.trigger == "eager" {
                    " (eager)"
                } else {
                    " (idle)"
                }
            ),
        )
        .field(
            "keep recent",
            format!(
                "{} message(s) after a compaction",
                configuration.compaction.keep_recent
            ),
        )
        .field(
            "catalogue",
            format!("{provider_count} providers · {model_count} models"),
        )
        .field("catalogue source", source_label(source))
        .field(
            "reasoning",
            format!(
                "{} effort(s) available",
                spec.capabilities.reasoning.efforts.len()
            ),
        )
        .field("workspace", &workspace)
        .field("config", &config_label)
        .field("project config", &project_config)
        .field("database", database.display().to_string())
        .field("sandbox", &sandbox);
    panel.render(style)
}

/// Prints the MCP servers that connected, and the ones that did not.
///
/// A server that failed to start leaves its tools absent, so the reason is
/// named here rather than left to the log.
pub(crate) fn debug_mcp(style: Style, service: &Service) -> Vec<String> {
    let connected = service.mcp_status();
    let failures = service.mcp_failures();
    if connected.is_empty() && failures.is_empty() {
        return Vec::new();
    }
    let mut panel = DebugPanel::new("server", "tools");
    for server in &connected {
        panel.field(
            format!("{} · {}", server.name, server.command),
            server.tools.len().to_string(),
        );
        // One row per tool: the names are what a person asks the model for, and
        // a count leaves them to be guessed.
        for tool in &server.tools {
            panel.field(format!("  {tool}"), "");
        }
    }
    for (name, reason) in &failures {
        panel.field(name.clone(), format!("failed: {reason}"));
    }
    panel.render(style)
}

/// Prints the static prompt size in characters. Tokens are provider-reported,
/// so no estimate is made here.
pub(crate) fn debug_prompt_size(style: Style, service: &Service, source: &str) -> Vec<String> {
    let size = match service.prompt_size() {
        Ok(size) => size,
        Err(error) => {
            return vec![style.yellow(format!("debug: could not measure the prompt: {error}"))];
        }
    };
    let mut lines = vec![debug_heading(style, "debug: prompt material")];
    let mut panel = DebugPanel::new("part", "size");
    panel
        .field("source", source)
        .field("system", format!("{} chars", size.system_chars));
    if let Some(path) = &size.project_path {
        panel.field(
            "project",
            format!("{} chars ({})", size.project_chars, path.display()),
        );
    }
    panel
        .field("tools", format!("{} chars", size.tool_chars))
        .field("total", format!("{} chars", size.total_chars()));
    lines.extend(panel.render(style));
    lines
}

/// Prints the selected model's capabilities and catalogue metadata.
pub(crate) fn debug_model(style: Style, spec: &ModelSpec, model: Option<&Model>) -> Vec<String> {
    let capabilities = &spec.capabilities;
    let mut lines = vec![debug_heading(style, "debug: model")];
    let mut panel = DebugPanel::new("capability", "value");
    panel.field("reference", spec.label());
    if let Some(model) = model {
        if !model.name.is_empty() {
            panel.field("name", &model.name);
        }
        if let Some(family) = &model.family {
            panel.field("family", family);
        }
        if let Some(description) = &model.description {
            panel.rule().field("about", description);
        }
    } else {
        panel.rule().field(
            "catalogue",
            "not found; the capabilities below are the config/fallback ones",
        );
    }
    panel
        .rule()
        .field(
            "wire",
            format!("{} · provider {}", spec.wire.family(), spec.provider),
        )
        .field("context", token_limit_label(capabilities.max_context))
        .field("output", token_limit_label(capabilities.max_output))
        .field(
            "modalities",
            format!(
                "input {} · output {}",
                modality_set(capabilities.modalities.input.keys()),
                modality_set(capabilities.modalities.output.iter())
            ),
        )
        .field("tools", tool_kind_set(&capabilities.tool_kinds))
        .field("reasoning", reasoning_capabilities(&capabilities.reasoning))
        .field(
            "streaming",
            capabilities
                .streaming
                .map_or_else(|| "-".to_owned(), |value| streaming_name(value).to_owned()),
        )
        .field("cache", cache_name(capabilities.cache))
        .field("features", feature_set(&capabilities.features))
        .field(
            "structured",
            structured_set(&capabilities.structured_output),
        );
    if let Some(model) = model {
        let mut metadata = Vec::new();
        if let Some(status) = &model.status {
            metadata.push(format!("status {status}"));
        }
        if let Some(knowledge) = &model.knowledge {
            metadata.push(format!("knowledge {knowledge}"));
        }
        if let Some(released) = &model.release_date {
            metadata.push(format!("released {released}"));
        }
        if let Some(updated) = &model.last_updated {
            metadata.push(format!("updated {updated}"));
        }
        if let Some(open_weights) = model.open_weights {
            metadata.push(format!("open weights {}", yes_no(open_weights)));
        }
        if model.attachment {
            metadata.push("attachments".to_owned());
        }
        if let Some(temperature) = model.temperature {
            metadata.push(format!("temperature {}", yes_no(temperature)));
        }
        if !metadata.is_empty() {
            panel.rule().field("catalogue", metadata.join(" · "));
        }
    }
    lines.extend(panel.render(style));
    lines
}

/// Prints the reasoning setting the session is actually using.
pub(crate) fn debug_reasoning(style: Style, reasoning: &Reasoning) -> Vec<String> {
    let mut lines = vec![debug_heading(style, "debug: reasoning")];
    let mut panel = DebugPanel::new("field", "value");
    panel.field("effective", reasoning_setting_label(reasoning));
    lines.extend(panel.render(style));
    lines
}

/// Names the reasoning setting in the words the command line accepts.
pub(crate) fn reasoning_setting_label(reasoning: &Reasoning) -> String {
    match reasoning {
        Reasoning::Automatic => "auto".to_owned(),
        Reasoning::Disabled => "off".to_owned(),
        Reasoning::Effort { effort } => effort.clone(),
        Reasoning::Budget { tokens } => format!("budget {tokens} tokens"),
    }
}

/// Formats a token limit without guessing an unknown value.
pub(crate) fn token_limit_label(limit: Option<u64>) -> String {
    limit.map_or_else(|| "unknown".to_owned(), |value| format!("{value} tokens"))
}
