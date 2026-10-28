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

use std::io::IsTerminal as _;
use std::path::Path;

use norte_code_config::{Config, update_user_default};
use norte_code_protocol::{Phase, SessionKey};
use norte_code_store_sqlite::{SessionDirectory as _, SqliteStore, StoredSession};

use crate::select::{self, Item, Outcome};
use crate::style::Style;

/// How many providers the picker names.
///
/// The configured ones come first and are never cut; the catalogue fills what is
/// left. Anything past the bound is reachable by name, and the count says how
/// many were left out.
const LISTED_PROVIDERS: usize = 20;

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
    /// The model the picker chose, or `None` when the configuration already
    /// named one and nothing had to be asked.
    pub(crate) model: Option<String>,
}

/// One session, as the startup list shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionRow {
    /// The logical key.
    pub(crate) key: String,
    /// Its label, when one was set.
    pub(crate) title: Option<String>,
    /// The `provider/model` it last used.
    pub(crate) model: String,
    /// Where it is, as the stored phase names it.
    pub(crate) phase: Phase,
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
        match self.title.as_deref().map(str::trim) {
            Some(title) if !title.is_empty() => format!("{title} ({})", short(&self.key)),
            _ => short(&self.key).to_owned(),
        }
    }

    /// What the state of the session is worth saying at startup.
    ///
    /// A session that is merely idle says nothing: "ready" on every row is a
    /// column of noise. The states worth a word are the ones where opening the
    /// session does something that was not asked for — continuing unfinished
    /// work — or where it is waiting on a person.
    pub(crate) fn state(&self) -> Option<&'static str> {
        if self.unfinished {
            return Some("unfinished");
        }
        match self.phase {
            Phase::AwaitingApproval | Phase::AwaitingTool => Some("waiting"),
            Phase::Streaming => Some("interrupted"),
            Phase::Idle => None,
        }
    }
}

/// The key as the footer and the lists shorten it.
fn short(key: &str) -> &str {
    key.get(..8).unwrap_or(key)
}

/// The sessions in the local database, newest first.
///
/// The repair runs here for the same reason `sessions_command` runs it: a
/// session a hard kill left `streaming` is not streaming now, and a list that
/// says otherwise offers to continue a turn that is already over.
pub(crate) async fn session_rows(path: &Path) -> Result<Vec<SessionRow>, String> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let store = SqliteStore::open(path)
        .map_err(|error| format!("could not open {}: {error}", path.display()))?;
    if let Err(error) = store.repair_interrupted().await {
        eprintln!("warning: could not repair interrupted sessions: {error}");
    }
    let sessions = store
        .sessions()
        .await
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    Ok(sessions.into_iter().map(row_of).collect())
}

/// One stored session as a row.
fn row_of(session: StoredSession) -> SessionRow {
    let phase = match session.phase.as_str() {
        "streaming" => Phase::Streaming,
        "awaiting_tool" => Phase::AwaitingTool,
        "awaiting_approval" => Phase::AwaitingApproval,
        _ => Phase::Idle,
    };
    SessionRow {
        // A phase a live turn writes is a phase that outlived it: no process is
        // streaming a session it has just opened, and an approval nobody is
        // waiting on is the same shape. Either one is work to continue.
        unfinished: matches!(phase, Phase::Streaming | Phase::AwaitingApproval),
        key: session.key,
        title: session.title,
        model: session.model,
        phase,
    }
}

/// Runs the flow: which session, then which model.
///
/// A session that already has a model is never asked about, and neither is a run
/// whose configuration already names a default, so the question is asked once
/// and the answer is written where the next run reads it.
pub(crate) async fn startup(
    config: &mut Config,
    catalog: &norte_code_catalog::Catalog,
    database: &Path,
    config_path: Option<&Path>,
    style: Style,
) -> Result<Startup, String> {
    let rows = session_rows(database).await?;
    let open = choose_session(&rows, style)?;
    let saved = match &open {
        // Continuing a session keeps the model it was saved with: replacing it
        // here would be a decision nobody asked for, and `--model` and `/model`
        // are how that is changed on purpose.
        Open::Existing { key, .. } => rows
            .iter()
            .find(|row| row.key == key.as_str())
            .map(|row| row.model.clone()),
        Open::New => None,
    };
    let model = match saved.filter(|model| !model.is_empty()) {
        Some(model) => Some(model),
        None if config.default_model.is_some() => None,
        None => Some(choose_model(config, catalog, config_path, style)?),
    };
    Ok(Startup { open, model })
}

/// Asks which session to open.
///
/// Nothing saved yet skips the question: a first run has exactly one thing it
/// can do, and a menu whose only working entry is "new" is a keystroke asked for
/// nothing.
pub(crate) fn choose_session(rows: &[SessionRow], style: Style) -> Result<Open, String> {
    if rows.is_empty() {
        return Ok(Open::New);
    }
    let items = session_items(rows);
    match select::run(items, "which session?", style)? {
        // A bare Enter took the new session, and Escape means the same thing: it
        // is the one choice that needs nothing typed, so it is what an
        // unanswered menu means.
        Outcome::Chosen(0) | Outcome::Left => Ok(Open::New),
        // The second entry is the newest session, whatever its key is.
        Outcome::Chosen(1) => Ok(open_of(&rows[0])),
        Outcome::Chosen(index) => Ok(open_of(&rows[index - 2])),
        // A name that matched no entry is a session key the list did not show,
        // typed from an older footer. It is passed through rather than checked
        // here, which would need a second reader of the same table; the service
        // resolves it at the first message and says so if it is unknown.
        Outcome::Typed(typed) => {
            let typed = typed.trim();
            if typed.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
                return Ok(Open::Existing {
                    key: SessionKey::new(typed.to_owned()),
                    unfinished: false,
                });
            }
            Err(format!("`{typed}` names no session"))
        }
    }
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
        items[1] = items[1].clone().noted(state);
    }
    for row in rows {
        let mut item = Item::new(row.label(), false).searched_by(row.key.clone());
        if let Some(state) = row.state() {
            item = item.noted(state);
        }
        items.push(item);
    }
    items
}

/// Asks which provider and which model a new session should use.
///
/// The providers a person actually has are the ones named in the configuration:
/// they carry a key, an endpoint and a wire, so they are listed first and the
/// catalogue follows. The catalogue is the whole of models.dev — hundreds of
/// providers — so printing it all would bury the two or three that matter under
/// a screenful nobody reads. The list is what the cursor walks; a name the cursor
/// does not reach is typed into the filter, which is what narrows the list and,
/// when it matches nothing, is the name itself.
#[allow(clippy::too_many_lines)] // one arm per question, which is the dialogue.
pub(crate) fn choose_model(
    config: &mut Config,
    catalog: &norte_code_catalog::Catalog,
    config_path: Option<&Path>,
    style: Style,
) -> Result<String, String> {
    let mut configured: Vec<String> = config.providers.keys().cloned().collect();
    configured.sort();
    let mut from_catalogue: Vec<String> = catalog
        .providers
        .keys()
        .filter(|provider| !config.providers.contains_key(*provider))
        .cloned()
        .collect();
    from_catalogue.sort();
    eprintln!("{}", style.bold("no default model is configured"));
    if configured.is_empty() && from_catalogue.is_empty() {
        return Err(
            "no model selected: the catalogue is empty and no provider is configured; \
             name one with `--model` or set default_model in the config"
                .to_owned(),
        );
    }
    // The configured providers are never cut. They are the ones with a key and
    // an endpoint, which are the two things the catalogue cannot supply, and a
    // bound that hid one would hide the only choice that works.
    let mut providers = configured.clone();
    providers.extend(
        from_catalogue
            .iter()
            .take(LISTED_PROVIDERS.saturating_sub(configured.len()))
            .cloned(),
    );
    let items: Vec<Item> = providers
        .iter()
        .map(|provider| {
            let source = if configured.contains(provider) {
                "config"
            } else {
                "catalogue"
            };
            Item::new(provider.clone(), false).noted(source)
        })
        .collect();
    let hidden = from_catalogue
        .len()
        .saturating_sub(providers.len().saturating_sub(configured.len()));
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
            configured
                .iter()
                .chain(from_catalogue.iter())
                .find(|provider| provider.eq_ignore_ascii_case(typed))
                .cloned()
                .ok_or_else(|| format!("`{typed}` names no provider"))?
        }
        // With no provider there is no model to ask about, and refusing is what
        // the numbered prompt did with an unfinished answer.
        Outcome::Left => return Err("no provider chosen".to_owned()),
    };

    let mut models: Vec<String> = catalog
        .provider(&provider)
        .map(|entry| entry.models.keys().cloned().collect())
        .unwrap_or_default();
    models.sort();
    if models.is_empty() {
        // A provider the catalogue does not list has no models to offer: it is a
        // configured endpoint — a private gateway, a local server — and the model
        // is whatever that endpoint serves, so the name is typed.
        eprintln!(
            "  {}",
            style.dim(format!(
                "{provider} is not in the catalogue; type the model it serves"
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
                .provider(&provider)
                .and_then(|entry| entry.models.get(model))
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

/// Writes the choice into the config file and returns the reference.
///
/// A file that cannot be written is not a reason to refuse the choice: the
/// session is usable now, and the reference is printed so the run continues with
/// a line to paste. Either way the reference is what the caller resolves, so the
/// session opens on the model that was chosen.
fn remember(
    config: &mut Config,
    config_path: Option<&Path>,
    provider: &str,
    model: &str,
    style: Style,
) -> String {
    let reference = format!("{provider}/{model}");
    let path = config_path
        .map(Path::to_path_buf)
        .or_else(norte_code_config::user_config_path);
    let Some(path) = path else {
        eprintln!(
            "{}",
            style.yellow(format!(
                "nowhere to write the choice; set default_provider to `{provider}` and default_model to `{model}` yourself"
            ))
        );
        return reference;
    };
    match update_user_default(&path, provider, model) {
        Ok(()) => {
            config.default_provider = Some(provider.to_owned());
            config.default_model = Some(model.to_owned());
            eprintln!("{}", style.dim(format!("saved to {}", path.display())));
        }
        Err(error) => eprintln!(
            "{}",
            style.yellow(format!(
                "could not write {}: {error}; set default_provider to `{provider}` and default_model to `{model}` yourself",
                path.display()
            ))
        ),
    }
    reference
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
            model: "river/deepseek".to_owned(),
            phase,
            unfinished,
        }
    }

    fn stored(phase: &str) -> StoredSession {
        StoredSession {
            key: "aaaaaaaa-0000-0000-0000-000000000000".to_owned(),
            north: norte::SessionId::new(),
            title: None,
            model: "openai/gpt-5".to_owned(),
            created_at: chrono::Utc::now(),
            phase: phase.to_owned(),
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
        assert_eq!(items[2].label, "7c19ab22");
        assert_eq!(items[2].note.as_deref(), Some("unfinished"));
        // A titled session leads with the title and keeps its key beside it, so
        // both what a person calls it and what they would type are on the row.
        assert_eq!(items[3].label, "browser work (03fe0fad)");
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
        assert_eq!(last.label, "0000001d");
        assert!(
            last.search.contains("0000001d-0000"),
            "the whole key is searchable: {}",
            last.search
        );
    }

    #[test]
    fn a_row_is_named_by_its_title_or_its_short_key() {
        let named = row("03fe0fad-a2ee", Some("browser work"), Phase::Idle, false);
        assert_eq!(named.label(), "browser work (03fe0fad)");
        assert_eq!(named.state(), None);
        let untitled = row("03fe0fad-a2ee", None, Phase::Idle, false);
        assert_eq!(untitled.label(), "03fe0fad");
        // A blank title is not a name: the key is all it has.
        let blank = row("03fe0fad-a2ee", Some("   "), Phase::Idle, false);
        assert_eq!(blank.label(), "03fe0fad");
    }

    #[test]
    fn a_session_with_something_to_continue_says_so() {
        let unfinished = row("aaaaaaaa", None, Phase::Streaming, true);
        assert_eq!(unfinished.state(), Some("unfinished"));
        let waiting = row("aaaaaaaa", None, Phase::AwaitingApproval, true);
        assert_eq!(waiting.state(), Some("unfinished"));
        let stale = row("aaaaaaaa", None, Phase::AwaitingTool, false);
        assert_eq!(stale.state(), Some("waiting"));
    }

    #[test]
    fn a_stored_phase_becomes_the_phase_the_client_shows() {
        assert_eq!(row_of(stored("streaming")).phase, Phase::Streaming);
        assert_eq!(row_of(stored("awaiting_tool")).phase, Phase::AwaitingTool);
        assert_eq!(
            row_of(stored("awaiting_approval")).phase,
            Phase::AwaitingApproval
        );
        // Anything else starts idle, which is what the service does with a phase
        // it does not recognise.
        assert_eq!(row_of(stored("idle")).phase, Phase::Idle);
        assert_eq!(row_of(stored("nonsense")).phase, Phase::Idle);
        // A turn in flight outlived its process, so there is work to continue; a
        // tool that was about to run is not the same thing.
        assert!(row_of(stored("streaming")).unfinished);
        assert!(row_of(stored("awaiting_approval")).unfinished);
        assert!(!row_of(stored("idle")).unfinished);
        assert!(!row_of(stored("awaiting_tool")).unfinished);
    }

    #[test]
    fn the_choice_is_written_to_the_file_it_was_given() {
        let dir = std::env::temp_dir().join(format!("norte-startup-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("config.json");
        std::fs::write(&path, r#"{"version":1,"cost":{"show":false}}"#).expect("write");
        let mut config = Config::from_json("{}").expect("config").config;
        assert_eq!(config.default_model, None);
        assert_eq!(config.default_provider, None);
        let reference = remember(
            &mut config,
            Some(&path),
            "river",
            "deepseek-ai/DeepSeek-V4.1-Flash",
            Style::plain(),
        );
        assert_eq!(reference, "river/deepseek-ai/DeepSeek-V4.1-Flash");
        // The two are written apart, and the model keeps its slash: a name is a
        // string and the provider is its own key.
        assert_eq!(config.default_provider.as_deref(), Some("river"));
        assert_eq!(
            config.default_model.as_deref(),
            Some("deepseek-ai/DeepSeek-V4.1-Flash")
        );
        // The rest of the file survives, and what was written is a config the
        // next run can load.
        let written = std::fs::read_to_string(&path).expect("read");
        assert!(written.contains("\"show\": false"), "{written}");
        let reloaded = Config::from_json(&written).expect("reload");
        assert_eq!(reloaded.config.default_provider.as_deref(), Some("river"));
        assert_eq!(
            reloaded.config.default_model.as_deref(),
            Some("deepseek-ai/DeepSeek-V4.1-Flash")
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
