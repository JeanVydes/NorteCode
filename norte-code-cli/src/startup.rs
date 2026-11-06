#![allow(dead_code)]
//! The startup flow of a bare `norte-code`.
//!
//! `run` and `resume` both start from a decision the command line already made:
//! a prompt for a new session, a key for an existing one. A bare `norte-code`
//! has made no decision, so this module asks for one — begin a new session,
//! continue the last, or pick from the list — and settles the model at the same
//! moment when the configuration does not already name one.
//!
//! The order is not free. The model has to be chosen *before* the service is
//! built, because the service is built around a spec and there is no spec
//! without a model; the session has to exist before the first message, because a
//! message is what creates a session when none was chosen. So the flow runs
//! first and hands back both answers, and the caller builds the service and then
//! creates the session.
//!
//! It reads its own input instead of sharing the editor, because it runs before
//! the editor exists: the editor takes the terminal over — raw mode, a scroll
//! region, a pinned footer — and a menu that borrowed it would have to hand it
//! back before the session it is choosing for had started. It puts the terminal
//! in raw mode for the length of the question and restores it after, which is
//! what makes an arrow key arrive as an arrow key instead of as three bytes.

use norte_code_client::AgentClient;
use norte_code_protocol::config::Config;
#[cfg(test)]
use norte_code_protocol::sqlite::StoredSession;
use norte_code_protocol::{Phase, SessionInfo, SessionKey};
use std::io::IsTerminal as _;
use std::path::Path;

use crate::models::ModelChoice;
use crate::select::{self, Item, Outcome};
use crate::style::Style;

/// What the person asked the process to open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Open {
    /// A session with no history, to be created by the caller.
    New,
    /// A session that already exists, and whether it has work to continue.
    Existing {
        /// The session.
        key: SessionKey,
        /// Whether a turn was left unfinished, which a resume picks back up.
        unfinished: bool,
    },
}

/// What the startup flow decided.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Startup {
    /// Which session to open.
    pub(crate) open: Open,
    /// The provider and the model the picker chose, or `None` when the
    /// configuration already named them and nothing had to be asked.
    pub(crate) choice: Option<ModelChoice>,
}

/// The state of a session worth saying at startup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SessionState {
    /// A turn was interrupted and can be continued.
    Unfinished,
    /// The session is waiting on a tool.
    Waiting,
    /// The session was interrupted mid-stream.
    Interrupted,
}

impl SessionState {
    /// The word a person reads for this state.
    #[must_use]
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            SessionState::Unfinished => "unfinished",
            SessionState::Waiting => "waiting",
            SessionState::Interrupted => "interrupted",
        }
    }
}

/// One session, as the startup list shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionRow {
    /// The logical key.
    pub(crate) key: String,
    /// Its label, when one was set.
    pub(crate) title: Option<String>,
    /// The provider it last used.
    pub(crate) provider: String,
    /// The model it last used.
    pub(crate) model: String,
    /// Where it is, as the stored phase names it.
    pub(crate) phase: Phase,
    /// When the session was created.
    pub(crate) created_at: chrono::DateTime<chrono::Utc>,
    /// When the conversation was last updated.
    pub(crate) updated_at: chrono::DateTime<chrono::Utc>,
    /// Tokens of context the session last used.
    pub(crate) context_used: u64,
    /// Whether a turn was interrupted and can be continued.
    pub(crate) unfinished: bool,
}

impl SessionRow {
    /// The name a row is listed under: its title, or the start of its key.
    ///
    /// The footer names a session by the first eight characters of its key, so
    /// that is the name a person types back; a title is what they wrote to
    /// recognise it by, and both are worth seeing.
    pub(crate) fn label(&self) -> String {
        let key = self.key.get(..12).unwrap_or(&self.key);
        let date = self.updated_at.format("%Y-%m-%d %H:%M").to_string();
        let stats = if self.context_used > 0 {
            format!(" · {} ctx", self.context_used)
        } else {
            String::new()
        };
        match self.title.as_deref().map(str::trim) {
            Some(title) if !title.is_empty() => {
                format!("{title}  ({key}…)  {date}{stats}")
            }
            _ => format!("{key}…  {date}{stats}"),
        }
    }

    /// What the state of the session is worth saying at startup.
    ///
    /// A session that is merely idle says nothing: "ready" on every row is a
    /// column of noise. The states worth a word are the ones where opening the
    /// session does something that was not asked for — continuing unfinished
    /// work — or where it is waiting on a person.
    pub(crate) fn state(&self) -> Option<SessionState> {
        if self.unfinished {
            return Some(SessionState::Unfinished);
        }
        match self.phase {
            Phase::AwaitingTool => Some(SessionState::Waiting),
            Phase::Streaming => Some(SessionState::Interrupted),
            Phase::Idle => None,
        }
    }
}

/// The sessions in the local database, newest first.
///
/// The repair runs here for the same reason `sessions_command` runs it: a
/// session a hard kill left `streaming` is not streaming now, and a list that
/// says otherwise offers to continue a turn that is already over.
pub(crate) async fn session_rows(client: &dyn AgentClient) -> Result<Vec<SessionRow>, String> {
    let sessions = client
        .list_sessions()
        .await
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(norte_code_client::SessionSummary::into_session_info)
        .collect::<Vec<_>>();
    let mut rows = sessions.into_iter().map(row_of_info).collect::<Vec<_>>();
    rows.sort_by_key(|row| std::cmp::Reverse(row.updated_at));
    Ok(rows)
}

/// One stored session as a row.
fn row_of_info(session: SessionInfo) -> SessionRow {
    let phase = session.phase;
    let created_at = session
        .created_at
        .as_deref()
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(chrono::Utc::now);
    SessionRow {
        unfinished: matches!(phase, Phase::Streaming),
        key: session.session.as_str().to_owned(),
        title: session.title,
        provider: session.provider.unwrap_or_default(),
        model: session.model.unwrap_or_default(),
        phase,
        created_at,
        updated_at: created_at,
        context_used: 0,
    }
}

#[cfg(test)]
fn row_of(session: StoredSession, updated_at: chrono::DateTime<chrono::Utc>) -> SessionRow {
    SessionRow {
        unfinished: matches!(session.phase, Phase::Streaming),
        key: session.key,
        title: session.title,
        provider: session.provider,
        model: session.model,
        phase: session.phase,
        created_at: session.created_at,
        updated_at,
        context_used: session
            .stats
            .get("context_used")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0),
    }
}

/// Runs the flow: which session, then which model.
///
/// A session that already has a model is never asked about, and neither is a run
/// whose configuration already names a default, so the question is asked once
/// and the answer is written where the next run reads it.
pub(crate) async fn startup(
    _config: &mut Config,
    _catalog: &norte_code_protocol::catalog::Catalog,
    client: &dyn AgentClient,
    _config_path: Option<&Path>,
    _style: Style,
) -> Result<Startup, String> {
    // The default entrypoint is a new session: the session picker moved to
    // the `/sessions` command, so a bare run no longer stops on a menu.
    let _ = session_rows(client).await?;
    let open = Open::New;
    // No terminal picker any more: when no default is configured the interface
    // opens without a model and the person chooses one with `/models`.
    let choice: Option<ModelChoice> = None;
    Ok(Startup { open, choice })
}

fn open_of(row: &SessionRow) -> Open {
    Open::Existing {
        key: SessionKey::new(row.key.clone()),
        unfinished: row.unfinished,
    }
}

/// The rows the session menu draws: the two fixed entries, then the sessions.
///
/// The order is the contract the caller reads back: entry zero is a new session
/// and entry one is the newest saved one, and the sessions follow in order, so
/// `outcome - 2` is an index into `rows`. Every session is a row, and the menu's
/// own window is what bounds what is drawn: a filter has to be able to find a
/// session that is not on the first screen, and it only searches the rows it was
/// given — a list cut to ten here was a session that could not be typed to.
fn session_items(rows: &[SessionRow]) -> Vec<Item> {
    let mut items = vec![
        Item::new("New session", true),
        Item::new("Last session", true),
    ];
    if let Some(state) = rows[0].state() {
        // The state belongs to the row it describes, and that row is found by
        // its place: the second entry is the newest session.
        items[1] = items[1].clone().noted(state.as_str());
    }
    for row in rows {
        let mut item = Item::new(row.label(), false).searched_by(row.key.clone());
        if let Some(state) = row.state() {
            item = item.noted(state.as_str());
        }
        items.push(item);
    }
    items
}

/// Asks which provider and which model a new session should use.
///
/// The catalogue is the whole of models.dev — hundreds of providers — so printing
/// it all would bury the two or three that matter under a screenful nobody reads.
/// The list is what the cursor walks; a name the cursor does not reach is typed
/// into the filter, which is what narrows the list and, when it matches nothing,
/// is the name itself. A provider's key is resolved after the choice, from the
/// environment or `secrets.json`.
#[allow(clippy::too_many_lines)] // one arm per question, which is the dialogue.
#[allow(dead_code)]
pub(crate) fn choose_model(
    config: &mut Config,
    catalog: &norte_code_protocol::catalog::Catalog,
    config_path: Option<&Path>,
    style: Style,
) -> Result<ModelChoice, String> {
    // One list and not two: the catalogue is the only place a provider comes
    // from, so there is nothing to merge and nothing to prefer.
    let mut providers: Vec<String> = catalog.providers().keys().cloned().collect();
    providers.sort();
    eprintln!("{}", style.bold("no default model is configured"));
    if providers.is_empty() {
        return Err(
            "no model selected: the server catalogue defines no provider; \
             configure one on the server, or name a model with `--model`"
                .to_owned(),
        );
    }
    let items: Vec<Item> = providers
        .iter()
        .map(|provider| {
            // What the row says beside the name is the wire a session will open
            // on when nobody picks another, which is the one fact a person
            // choosing between providers wants before the choice.
            let wire = catalog
                .provider(provider)
                .and_then(|entry| entry.preferred_wire())
                .map_or_else(
                    || "no wire".to_owned(),
                    |wire| wire.schema.label().to_owned(),
                );
            Item::new(provider.clone(), false).noted(wire)
        })
        .collect();
    let hidden = catalog.providers().len().saturating_sub(providers.len());
    if hidden > 0 {
        eprintln!(
            "{}",
            style.dim(format!(
                "{hidden} more provider(s) in the catalogue; type a name to choose one"
            ))
        );
    }
    let provider = match select::run(items, "which provider?", style)? {
        Outcome::Chosen(index) => providers[index].clone(),
        // A name the list did not show is still a name: the catalogue is the
        // authority on what exists, and a provider behind the bound is reached
        // by what it is called.
        Outcome::Typed(typed) => {
            let typed = typed.trim();
            providers
                .iter()
                .find(|provider| provider.eq_ignore_ascii_case(typed))
                .cloned()
                .ok_or_else(|| format!("`{typed}` names no provider"))?
        }
        // With no provider there is no model to ask about, and refusing is what
        // the numbered prompt did with an unfinished answer.
        Outcome::Left => return Err("no provider chosen".to_owned()),
    };

    let mut models: Vec<String> = catalog
        .models_of(&provider)
        .map(|entry| entry.id.clone())
        .collect();
    models.sort();
    if models.is_empty() {
        // A provider with no models is a server-side catalogue inconsistency.
        eprintln!(
            "  {}",
            style.dim(format!(
                "{provider} lists no models in the server catalogue; type the model it serves"
            ))
        );
        let model = prompt_line("model> ", style)?.ok_or_else(|| "no model chosen".to_owned())?;
        let model = model.trim();
        if model.is_empty() {
            return Err(format!("no model named for provider `{provider}`"));
        }
        return Ok(remember(config, config_path, &provider, model, style));
    }

    let items: Vec<Item> = models
        .iter()
        .map(|model| {
            let name = catalog
                .find(&provider, model)
                .map(|entry| entry.name.clone())
                .unwrap_or_default();
            let item = Item::new(model.clone(), false);
            if name.is_empty() {
                item
            } else {
                item.noted(name)
            }
        })
        .collect();
    let model = match select::run(items, &format!("which model of {provider}?"), style)? {
        Outcome::Chosen(index) => models[index].clone(),
        // A name past the bound is still a name the catalogue knows, and one it
        // does not know is still what the endpoint answers to.
        Outcome::Typed(typed) => typed.trim().to_owned(),
        Outcome::Left => return Err("no model chosen".to_owned()),
    };
    if model.is_empty() {
        return Err(format!("no model named for provider `{provider}`"));
    }
    Ok(remember(config, config_path, &provider, &model, style))
}

/// Writes the choice into the config file and returns it.
///
/// A file that cannot be written is not a reason to refuse the choice: the
/// session is usable now, and the two names are printed so the run continues
/// with the keys to set. Either way the pair is what the caller resolves, so the
/// session opens on the provider and the model that were chosen.
fn remember(
    _config: &mut Config,
    _config_path: Option<&Path>,
    provider: &str,
    model: &str,
    style: Style,
) -> ModelChoice {
    let choice = ModelChoice::new(provider, model);
    eprintln!(
        "{}",
        style.dim(format!(
            "selected {provider}/{model}; persistent defaults are managed by the server"
        ))
    );
    choice
}

/// Whether the flow can ask: every question it has needs a terminal.
pub(crate) fn has_terminal() -> bool {
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

/// Prints a question and reads one line from the terminal.
///
/// `None` means the input ended, which is not the same as an empty line: the
/// first is a terminal that cannot answer and the second is a person accepting
/// what the prompt suggested.
///
/// It is the one question the flow still asks by line, and it has to: a provider
/// the catalogue does not list has no entries to walk, so the model it serves is
/// a name nobody could put on a list.
/// Asks for the key of the provider the run is about to use.
///
/// This is the pre-TUI path: there is no question box to draw a bullet in, so
/// the key is read with no echo. It only asks when a store is installed, the
/// provider is in the catalogue and none of its sources has a key. A
/// non-interactive client keeps the old behavior: the resolver reports the
/// missing variable and the run stops.
///
/// # Errors
///
/// Returns the resolver's missing-key message when the key is left empty or the
/// process has no terminal, and the read error when the terminal refuses it.
fn prompt_line(question: &str, style: Style) -> Result<Option<String>, String> {
    use std::io::Write as _;

    print!("{}", style.bold(question.to_owned()));
    std::io::stdout()
        .flush()
        .map_err(|error| error.to_string())?;
    let mut line = String::new();
    let read = std::io::stdin()
        .read_line(&mut line)
        .map_err(|error| error.to_string())?;
    if read == 0 {
        return Ok(None);
    }
    Ok(Some(line.trim_end_matches(['\n', '\r']).to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(key: &str, title: Option<&str>, phase: Phase, unfinished: bool) -> SessionRow {
        SessionRow {
            key: key.to_owned(),
            title: title.map(str::to_owned),
            provider: "river".to_owned(),
            model: "deepseek".to_owned(),
            phase,
            created_at: chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
            updated_at: chrono::DateTime::parse_from_rfc3339("2026-01-02T00:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
            context_used: 0,
            unfinished,
        }
    }

    fn stored(phase: &str) -> StoredSession {
        StoredSession {
            key: "aaaaaaaa-0000-0000-0000-000000000000".to_owned(),
            north: norte::SessionId::new(),
            title: None,
            provider: "openai".to_owned(),
            model: "gpt-5".to_owned(),
            created_at: chrono::Utc::now(),
            phase: match phase {
                "streaming" => Phase::Streaming,
                "awaiting_tool" => Phase::AwaitingTool,
                _ => Phase::Idle,
            },
            stats: serde_json::Value::Null,
            denied_tools: Vec::new(),
            allowed_tools: Vec::new(),
            compacted_from: None,
            workdir: None,
        }
    }

    #[test]
    fn the_menu_offers_the_new_session_the_last_one_and_the_saved_ones() {
        let rows = vec![
            row("7c19ab22-0000", None, Phase::Streaming, true),
            row("03fe0fad-a2ee", Some("browser work"), Phase::Idle, false),
        ];
        let items = session_items(&rows);
        // Two fixed entries and one row per session, in the order the rows
        // arrived in, which is newest first.
        assert_eq!(items.len(), 4);
        assert_eq!(items[0].label, "New session");
        assert!(items[0].fixed, "the fixed entries are drawn bold");
        assert_eq!(items[1].label, "Last session");
        // The state of the newest session is on the row it describes, rather
        // than only being reachable by opening it.
        assert_eq!(items[1].note.as_deref(), Some("unfinished"));
        // The newest session is the running one; the one after it is idle, and
        // an idle session says nothing: "ready" on every row is a column of
        // noise.
        assert!(items[2].label.starts_with("7c19ab22"), "{}", items[2].label);
        assert_eq!(items[2].note.as_deref(), Some("unfinished"));
        // A titled session leads with the title and keeps its key beside it, so
        // both what a person calls it and what they would type are on the row.
        assert!(
            items[3].label.starts_with("browser work"),
            "{}",
            items[3].label
        );
        assert_eq!(items[3].note, None);
        assert!(
            items[3].search.contains("03fe0fad-a2ee"),
            "the whole key is searchable, not just the eight shown: {}",
            items[3].search
        );
    }

    #[test]
    fn every_session_reaches_the_menu_so_the_filter_can_find_it() {
        // The menu draws a window and scrolls it, so a long list is cheap to
        // show; what it cannot do is search a row it was never given. The list
        // was cut to a fixed ten here, so typing the key of the eleventh session
        // matched nothing and the session looked absent though it existed. Every
        // row is handed over now, newest first.
        let rows: Vec<SessionRow> = (0..30)
            .map(|index| row(&format!("{index:08x}-0000"), None, Phase::Idle, false))
            .collect();
        let items = session_items(&rows);
        assert_eq!(
            items.len(),
            rows.len() + 2,
            "two fixed entries and every row"
        );
        // The last one is a row of the menu, and it is searchable by its key.
        let last = &items[items.len() - 1];
        assert!(last.label.starts_with("0000001d"), "{}", last.label);
        assert!(
            last.search.contains("0000001d-0000"),
            "the whole key is searchable: {}",
            last.search
        );
    }

    #[test]
    fn a_row_is_named_by_its_title_or_its_short_key() {
        let named = row("03fe0fad-a2ee", Some("browser work"), Phase::Idle, false);
        assert!(
            named.label().starts_with("browser work"),
            "{}",
            named.label()
        );
        assert_eq!(named.state(), None);
        let untitled = row("03fe0fad-a2ee", None, Phase::Idle, false);
        assert!(
            untitled.label().starts_with("03fe0fad"),
            "{}",
            untitled.label()
        );
        // A blank title is not a name: the key is all it has.
        let blank = row("03fe0fad-a2ee", Some("   "), Phase::Idle, false);
        assert!(blank.label().starts_with("03fe0fad"), "{}", blank.label());
    }

    #[test]
    fn a_session_with_something_to_continue_says_so() {
        let unfinished = row("aaaaaaaa", None, Phase::Streaming, true);
        assert_eq!(unfinished.state(), Some(SessionState::Unfinished));
        let stale = row("aaaaaaaa", None, Phase::AwaitingTool, false);
        assert_eq!(stale.state(), Some(SessionState::Waiting));
    }

    #[test]
    fn a_stored_phase_becomes_the_phase_the_client_shows() {
        assert_eq!(
            row_of(stored("streaming"), chrono::Utc::now()).phase,
            Phase::Streaming
        );
        assert_eq!(
            row_of(stored("awaiting_tool"), chrono::Utc::now()).phase,
            Phase::AwaitingTool
        );
        // Anything else starts idle, which is what the service does with a phase
        // it does not recognise.
        assert_eq!(
            row_of(stored("idle"), chrono::Utc::now()).phase,
            Phase::Idle
        );
        assert_eq!(
            row_of(stored("nonsense"), chrono::Utc::now()).phase,
            Phase::Idle
        );
        // A turn in flight outlived its process, so there is work to continue; a
        // tool that was about to run is not the same thing.
        assert!(row_of(stored("streaming"), chrono::Utc::now()).unfinished);
        assert!(!row_of(stored("idle"), chrono::Utc::now()).unfinished);
        assert!(!row_of(stored("awaiting_tool"), chrono::Utc::now()).unfinished);
    }

    #[test]
    fn the_choice_is_returned_without_writing_server_configuration() {
        let mut config = Config::default();
        let choice = remember(
            &mut config,
            None,
            "river",
            "deepseek-ai/DeepSeek-V4.1-Flash",
            Style::plain(),
        );
        // The pair is carried apart: the provider is its own key and the model
        // keeps its slash, so neither is ever taken from a joined string.
        assert_eq!(choice.provider, "river");
        assert_eq!(choice.model, "deepseek-ai/DeepSeek-V4.1-Flash");
        assert!(config.default_provider.is_none());
        assert!(config.default_model.is_none());
    }
}
