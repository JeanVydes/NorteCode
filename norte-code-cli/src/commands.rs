//! Non-interactive subcommands and shared configuration loading.

use super::{
    BTreeMap, BTreeSet, Catalog, Cli, Command, Config, CostArgs, CostSummary, DateTime, Duration,
    LoadOptions, Path, PathBuf, RequestRecord, ResumeArgs, SessionDirectory, SessionsArgs,
    SkillRegistry, SkillsArgs, SkillsCommand, Source, SqliteStore, Style, TelemetryArgs, Utc,
    by_day, by_session, by_turn, cache_dir, cache_percent_label, discovery_roots,
    find_project_config, human_tokens, list_models, load_catalog, new_session, open_session,
    resume_named, short, summarize, token_ratio, user_config_path,
};

pub(crate) async fn run(cli: Cli) -> Result<u8, String> {
    let config_path = cli.config.clone();
    let user_config = config_path.clone().or_else(user_config_path);
    let offline = cli.offline;
    let refresh = cli.refresh;
    // Where the work happens. The whole process moves, not just the tools:
    // everything downstream — the skills roots, the project instructions, the
    // tools, the sandbox — is built from the process's own directory, and one
    // move is what makes `norte-code ~/src/thing` mean what it says instead of
    // a root that disagrees with the shell it was typed in.
    //
    // It moves before the config is loaded, so a project config in the named
    // directory is the one that applies: `norte-code ./other` picking up this
    // directory's `.norte-code/config.json` would be the wrong project.
    if let Some(dir) = cli.dir.as_deref() {
        enter_workdir(dir)?;
    }
    let config = load_config(config_path)?;
    let verbose = resolve_verbosity(cli.verbose.as_deref(), &config);
    // A bare `norte-code` is the startup flow with nothing else on the line:
    // `None` is not a missing subcommand but the whole point of the command.
    let command = cli
        .command
        .unwrap_or_else(|| Command::Resume(ResumeArgs::bare()));
    match command {
        Command::Skills(args) => skills_command(&config, &args),
        Command::Sessions(args) => sessions_command(&args).await,
        Command::Cost(args) => cost_command(&config, &args).await,
        Command::Telemetry(args) => telemetry_command(&args).await,
        command => {
            let (catalog, source) = load_catalogue(&config, offline, refresh).await?;
            match command {
                // `new` names its model with a flag when it wants one and
                // falls back to `default_model` when it does not, so the flow
                // is not consulted: a session asked for on the command line is
                // not a session the menu should ask about.
                Command::New(args) => {
                    new_session(config, catalog, source, None, args, &verbose, user_config).await
                }
                Command::Resume(args) => match args.session.clone() {
                    // A named session is resumed even with no terminal, which is
                    // what a scripted client uses.
                    Some(key) => {
                        resume_named(config, catalog, source, key, args, &verbose, user_config)
                            .await
                    }
                    // No key: nothing to continue until someone says what, so
                    // the startup flow asks.
                    None => {
                        open_session(config, catalog, source, args, &verbose, user_config).await
                    }
                },
                Command::Models(args) => list_models(&config, &catalog, source, &args),
                Command::Skills(_)
                | Command::Sessions(_)
                | Command::Cost(_)
                | Command::Telemetry(_) => {
                    unreachable!("these commands do not need the catalogue")
                }
            }
        }
    }
}

pub(crate) fn skills_command(config: &Config, args: &SkillsArgs) -> Result<u8, String> {
    let workspace = std::env::current_dir().map_err(|error| error.to_string())?;
    let roots = discovery_roots(&workspace, &config.skills.paths, config.skills.interop);
    let registry = SkillRegistry::discover(&roots, config.skills.max_skills);
    match args.command {
        SkillsCommand::List { json } => {
            for skill in registry.skills() {
                if json {
                    let line = serde_json::json!({
                        "name": skill.name,
                        "description": skill.description,
                        "root": skill.root.display().to_string(),
                        "allowed_tools": skill.allowed_tools,
                    });
                    println!("{line}");
                } else {
                    println!(
                        "{}\t{}\t{}",
                        crate::text::terminal(&skill.name),
                        crate::text::terminal(&skill.root.display().to_string()),
                        crate::text::terminal(&skill.description)
                    );
                }
            }
            Ok(0)
        }
        SkillsCommand::Check => {
            for skill in registry.skills() {
                println!(
                    "ok\t{}\t{}\t{}",
                    crate::text::terminal(&skill.name),
                    crate::text::terminal(&skill.root.display().to_string()),
                    crate::text::terminal(&skill.description)
                );
            }
            for warning in registry.warnings() {
                eprintln!(
                    "warning: {}: {}",
                    crate::text::terminal(&warning.path.display().to_string()),
                    crate::text::terminal(&warning.message)
                );
            }
            if registry.warnings().is_empty() {
                Ok(0)
            } else {
                Ok(1)
            }
        }
    }
}

/// Where Norte Code keeps its database.
pub(crate) fn database_path() -> PathBuf {
    if let Some(data) = std::env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(data).join("norte-code").join("sessions.db");
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("norte-code")
            .join("sessions.db");
    }
    PathBuf::from(".norte-code").join("sessions.db")
}

pub(crate) async fn sessions_command(args: &SessionsArgs) -> Result<u8, String> {
    let path = database_path();
    if !path.exists() {
        if !args.json {
            println!("no sessions yet (the database is {})", path.display());
        }
        return Ok(0);
    }
    let store = SqliteStore::open(&path)
        .map_err(|error| format!("could not open {}: {error}", path.display()))?;
    if let Err(error) = store.repair_interrupted().await {
        eprintln!("warning: could not repair interrupted sessions: {error}");
    }
    let sessions = store
        .sessions()
        .await
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    for session in sessions {
        if args.json {
            let line = serde_json::json!({
                "key": session.key,
                "title": session.title,
                "model": session.model,
                "created_at": session.created_at.to_rfc3339(),
                "phase": session.phase,
            });
            println!("{line}");
        } else {
            println!(
                "{}\t{}\t{}\t{}",
                crate::text::terminal(&session.key),
                crate::text::terminal(session.title.as_deref().unwrap_or("(untitled)")),
                crate::text::terminal(&session.model),
                crate::text::terminal(&session.phase)
            );
        }
    }
    Ok(0)
}

pub(crate) async fn cost_command(config: &Config, args: &CostArgs) -> Result<u8, String> {
    if !config.cost.show {
        eprintln!("cost display is off (cost.show = false)");
        return Ok(0);
    }
    let path = database_path();
    if !path.exists() {
        println!("no cost records yet (the database is {})", path.display());
        return Ok(0);
    }
    let store = SqliteStore::open(&path)
        .map_err(|error| format!("could not open {}: {error}", path.display()))?;
    let records = store
        .costs(args.session.as_deref())
        .await
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    if records.is_empty() {
        println!("no cost records yet");
        return Ok(0);
    }
    let total = summarize(&records);
    let sessions = by_session(&records);
    let days = by_day(&records);
    let turns = by_turn(&records);
    if args.json {
        let sessions: Vec<_> = sessions
            .iter()
            .map(|(session, summary)| serde_json::json!({"session": session, "summary": summary}))
            .collect();
        let days: Vec<_> = days
            .iter()
            .map(|(day, summary)| serde_json::json!({"day": day, "summary": summary}))
            .collect();
        let turns: Vec<_> = turns
            .iter()
            .map(|((session, turn), summary)| {
                serde_json::json!({"session": session, "turn": turn, "summary": summary})
            })
            .collect();
        println!(
            "{}",
            serde_json::json!({
                "total": total,
                "sessions": sessions,
                "days": days,
                "turns": turns,
            })
        );
        return Ok(0);
    }
    let style = Style::stdout();
    println!("{}  {}", style.bold("total"), cost_summary(&total));
    if args.session.is_some() {
        for ((_, turn), summary) in &turns {
            println!("turn {turn:<4}     {}", cost_summary(summary));
        }
    } else if !args.daily {
        for (session, summary) in &sessions {
            println!(
                "{}  {}",
                style.bold(format!("session {}", short(session))),
                cost_summary(summary)
            );
        }
    }
    if !args.daily && args.session.is_none() {
        println!();
    }
    for (day, summary) in &days {
        println!("{}  {}", style.bold(day), cost_summary(summary));
    }
    Ok(0)
}

/// One line of token counts, reported cost and missing-cost count.
pub(crate) fn cost_summary(summary: &CostSummary) -> String {
    let mut parts = vec![
        format!("{} request(s)", summary.requests),
        format!("{} in", human_tokens(summary.input_tokens)),
        format!("{} out", human_tokens(summary.output_tokens)),
    ];
    if summary.cache_read_tokens > 0 && summary.input_tokens > 0 {
        parts.push(format!(
            "cache {}",
            cache_percent_label(summary.cache_read_tokens, summary.input_tokens)
        ));
    }
    parts.push(format!(
        "in:out {}",
        token_ratio(summary.input_tokens, summary.output_tokens)
    ));
    if summary.not_reported == summary.requests {
        parts.push("cost not reported".to_owned());
    } else {
        parts.push(format!("${:.6}", summary.reported_cost_usd));
        if summary.not_reported > 0 {
            parts.push(format!("{} not reported", summary.not_reported));
        }
    }
    parts.join(" · ")
}

/// Counts and sums one slice of the request telemetry.
#[derive(Default, Clone)]
pub(crate) struct RequestSummary {
    requests: u64,
    input_tokens: u64,
    output_tokens: u64,
    reasoning_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
    cache_hits: u64,
    tool_calls: u64,
    cost_usd: f64,
    unpriced: u64,
    errors: u64,
    cancelled: u64,
    durations: Vec<u64>,
    /// The time each request took to reach its first token.
    ttfts: Vec<u64>,
    /// The providers that served the requests, so a slice that mixes them says
    /// so.
    providers: BTreeSet<String>,
}

impl RequestSummary {
    pub(crate) fn add(&mut self, record: &RequestRecord) {
        self.requests += 1;
        self.input_tokens += record.input_tokens.unwrap_or(0);
        self.output_tokens += record.output_tokens.unwrap_or(0);
        self.reasoning_tokens += record.reasoning_tokens.unwrap_or(0);
        self.cache_read_tokens += record.cache_read_tokens.unwrap_or(0);
        self.cache_write_tokens += record.cache_write_tokens.unwrap_or(0);
        self.cache_hits += u64::from(record.cache_hit);
        self.tool_calls += u64::try_from(record.tool_calls.len()).unwrap_or(u64::MAX);
        if let Some(cost) = record.cost_usd {
            self.cost_usd += cost;
        } else {
            self.unpriced += 1;
        }
        match record.status.as_str() {
            "ok" => {}
            "cancelled" => self.cancelled += 1,
            _ => self.errors += 1,
        }
        self.durations.push(record.duration_ms);
        if let Some(first_token) = record.first_token_ms {
            self.ttfts.push(first_token);
        }
        self.providers.insert(record.provider.clone());
    }

    fn average_ms(&self) -> u64 {
        if self.requests == 0 {
            return 0;
        }
        self.durations.iter().sum::<u64>() / self.requests
    }

    fn p95_ms(&mut self) -> u64 {
        if self.durations.is_empty() {
            return 0;
        }
        self.durations.sort_unstable();
        let index = (self.durations.len() * 95 / 100).min(self.durations.len() - 1);
        self.durations[index]
    }

    /// The average time the requests took to reach their first token.
    fn average_ttft_ms(&self) -> u64 {
        if self.ttfts.is_empty() {
            return 0;
        }
        let count = u64::try_from(self.ttfts.len()).unwrap_or(u64::MAX);
        self.ttfts.iter().sum::<u64>() / count
    }

    /// The slowest first token of the slice, at the ninety-fifth percentile.
    fn p95_ttft_ms(&mut self) -> u64 {
        if self.ttfts.is_empty() {
            return 0;
        }
        self.ttfts.sort_unstable();
        let index = (self.ttfts.len() * 95 / 100).min(self.ttfts.len() - 1);
        self.ttfts[index]
    }

    fn cost_label(&self) -> String {
        if self.requests == 0 {
            return "no requests".to_owned();
        }
        if self.unpriced == self.requests {
            return "cost not reported".to_owned();
        }
        if self.unpriced > 0 {
            return format!("${:.6} · {} not reported", self.cost_usd, self.unpriced);
        }
        format!("${:.6}", self.cost_usd)
    }

    /// The providers that served the requests of the slice.
    fn provider_label(&self) -> String {
        if self.providers.is_empty() {
            return "–".to_owned();
        }
        self.providers.iter().cloned().collect::<Vec<_>>().join("+")
    }

    /// The tokens the slice moved: the prompt and the answer.
    fn token_total(&self) -> u64 {
        self.input_tokens.saturating_add(self.output_tokens)
    }

    /// The share of the prompt tokens the provider served from its cache.
    ///
    /// The share is weighted by tokens, not by requests: a request that hit
    /// the cache for a thousand tokens counts more than one that hit it for
    /// ten.
    pub(crate) fn cache_hit_percent(&self) -> Option<u64> {
        if self.input_tokens == 0 {
            return None;
        }
        Some((self.cache_read_tokens.saturating_mul(100) / self.input_tokens).min(100))
    }

    #[allow(clippy::cast_precision_loss)]
    pub(crate) fn json(&mut self) -> serde_json::Value {
        serde_json::json!({
            "requests": self.requests,
            "input_tokens": self.input_tokens,
            "output_tokens": self.output_tokens,
            "reasoning_tokens": self.reasoning_tokens,
            "cache_read_tokens": self.cache_read_tokens,
            "cache_write_tokens": self.cache_write_tokens,
            "cache_hits": self.cache_hits,
            "cache_hit_rate": if self.requests == 0 { 0.0 } else { self.cache_hits as f64 / self.requests as f64 },
            "tool_calls": self.tool_calls,
            "cost_usd": self.cost_usd,
            "unpriced": self.unpriced,
            "errors": self.errors,
            "cancelled": self.cancelled,
            "average_ms": self.average_ms(),
            "p95_ms": self.p95_ms(),
            "average_ttft_ms": self.average_ttft_ms(),
            "p95_ttft_ms": self.p95_ttft_ms(),
            "providers": self.providers.iter().cloned().collect::<Vec<_>>(),
        })
    }
}

#[allow(clippy::cast_precision_loss)]
pub(crate) fn duration_label(ms: u64) -> String {
    if ms >= 60_000 {
        format!("{:.1}m", ms as f64 / 60_000.0)
    } else if ms >= 1_000 {
        format!("{:.1}s", ms as f64 / 1_000.0)
    } else {
        format!("{ms}ms")
    }
}

pub(crate) fn bucket_label(at: DateTime<Utc>, bucket: &str) -> String {
    match bucket {
        "hour" => at.format("%Y-%m-%d %H:00").to_string(),
        "week" => at.format("%G-W%V").to_string(),
        "month" => at.format("%Y-%m").to_string(),
        "year" => at.format("%Y").to_string(),
        "total" => "total".to_owned(),
        _ => at.format("%Y-%m-%d").to_string(),
    }
}

fn top(groups: &BTreeMap<String, RequestSummary>) -> Vec<(&String, &RequestSummary)> {
    let mut top: Vec<_> = groups.iter().collect();
    // The busiest group first, measured in tokens: that is what the bar shows.
    top.sort_by(|left, right| {
        right
            .1
            .token_total()
            .cmp(&left.1.token_total())
            .then_with(|| left.0.cmp(right.0))
    });
    top
}

#[allow(clippy::too_many_lines)]
pub(crate) async fn telemetry_command(args: &TelemetryArgs) -> Result<u8, String> {
    let path = database_path();
    if !path.exists() {
        println!(
            "no request telemetry yet (the database is {})",
            path.display()
        );
        return Ok(0);
    }
    let store = SqliteStore::open(&path)
        .map_err(|error| format!("could not open {}: {error}", path.display()))?;
    let records = store
        .requests(args.session.as_deref())
        .await
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    let records: Vec<_> = records
        .into_iter()
        .filter(|record| args.provider.as_ref().is_none_or(|p| &record.provider == p))
        .filter(|record| args.model.as_ref().is_none_or(|m| &record.model == m))
        .collect();
    if records.is_empty() {
        println!("no matching request telemetry");
        return Ok(0);
    }
    let mut total = RequestSummary::default();
    let mut buckets: BTreeMap<String, RequestSummary> = BTreeMap::new();
    let mut providers: BTreeMap<String, RequestSummary> = BTreeMap::new();
    let mut models: BTreeMap<String, RequestSummary> = BTreeMap::new();
    let mut sessions: BTreeMap<String, RequestSummary> = BTreeMap::new();
    for record in &records {
        total.add(record);
        buckets
            .entry(bucket_label(record.started_at, &args.bucket))
            .or_default()
            .add(record);
        providers
            .entry(record.provider.clone())
            .or_default()
            .add(record);
        models
            .entry(format!("{}/{}", record.provider, record.model))
            .or_default()
            .add(record);
        sessions
            .entry(record.session.clone())
            .or_default()
            .add(record);
    }
    if args.json {
        let mut total_json = total.clone();
        let bucket_json: Vec<_> = buckets
            .iter_mut()
            .map(|(bucket, summary)| {
                serde_json::json!({
                    "bucket": bucket,
                    "summary": summary.json(),
                })
            })
            .collect();
        let provider_json: Vec<_> = top(&providers)
            .into_iter()
            .map(|(provider, summary)| {
                let mut summary = summary.clone();
                serde_json::json!({"provider": provider, "summary": summary.json()})
            })
            .collect();
        let model_json: Vec<_> = top(&models)
            .into_iter()
            .map(|(model, summary)| {
                let mut summary = summary.clone();
                serde_json::json!({"model": model, "summary": summary.json()})
            })
            .collect();
        let session_json: Vec<_> = top(&sessions)
            .into_iter()
            .map(|(session, summary)| {
                let mut summary = summary.clone();
                serde_json::json!({"session": session, "summary": summary.json()})
            })
            .collect();
        println!(
            "{}",
            serde_json::json!({
                "bucket": args.bucket,
                "total": total_json.json(),
                "series": bucket_json,
                "providers": provider_json,
                "models": model_json,
                "sessions": session_json,
            })
        );
        return Ok(0);
    }
    let style = Style::stdout();
    println!(
        "{}",
        telemetry_report(
            style, args, &total, &buckets, &providers, &models, &sessions
        )
    );
    Ok(0)
}

/// The width of the bar that shows how many tokens a group moved.
const BAR_WIDTH: usize = 16;

/// The telemetry report, as text.
///
/// The numbers a human reads are the ones that cost money or time: the
/// requests, the tokens, the time to the first token, the latency and the
/// cost. A bar shows how the group compares with the busiest one.
fn telemetry_report(
    style: Style,
    args: &TelemetryArgs,
    total: &RequestSummary,
    buckets: &BTreeMap<String, RequestSummary>,
    providers: &BTreeMap<String, RequestSummary>,
    models: &BTreeMap<String, RequestSummary>,
    sessions: &BTreeMap<String, RequestSummary>,
) -> String {
    let mut out = String::new();
    out.push_str(&style.bold("telemetry"));
    out.push_str("  ");
    out.push_str(&style.dim(format!(
        "{} requests · {} in · {} out · ${:.6}",
        total.requests,
        human_tokens(total.input_tokens),
        human_tokens(total.output_tokens),
        total.cost_usd
    )));
    out.push_str("\n\n");
    out.push_str(&telemetry_table(
        style,
        &format!("by {}", args.bucket),
        &buckets
            .iter()
            .map(|(bucket, summary)| (bucket.as_str(), summary))
            .collect::<Vec<_>>(),
        true,
    ));
    if args.session.is_none() {
        out.push_str(&telemetry_table(
            style,
            "by provider",
            &top(providers)
                .into_iter()
                .map(|(provider, summary)| (provider.as_str(), summary))
                .collect::<Vec<_>>(),
            false,
        ));
        out.push_str(&telemetry_table(
            style,
            "by model",
            &top(models)
                .into_iter()
                .map(|(model, summary)| (model.as_str(), summary))
                .collect::<Vec<_>>(),
            false,
        ));
    }
    // A session that barely moved is noise in the table: below one percent of
    // the total it is hidden, and the report says how many.
    let threshold = total.token_total() / 100;
    let mut visible = Vec::new();
    let mut hidden = 0usize;
    for (session, summary) in top(sessions) {
        if summary.token_total() < threshold {
            hidden += 1;
        } else {
            visible.push((short(session), summary));
        }
    }
    out.push_str(&telemetry_table(style, "by session", &visible, true));
    if hidden > 0 {
        out.push_str(&style.dim(format!("{hidden} session(s) hidden: below 1% of the total")));
        out.push_str("\n\n");
    }
    let mut notes = Vec::new();
    if let Some(percent) = total.cache_hit_percent() {
        notes.push(format!("{percent}% of the prompt from the cache"));
    }
    if total.tool_calls > 0 {
        notes.push(format!("{} tool call(s)", total.tool_calls));
    }
    if total.errors + total.cancelled > 0 {
        notes.push(format!(
            "{} error(s), {} cancelled",
            total.errors, total.cancelled
        ));
    }
    if !notes.is_empty() {
        out.push('\n');
        out.push_str(&style.dim(notes.join(" · ")));
        out.push('\n');
    }
    out
}

/// One telemetry table: a title and a row per group.
///
/// The rows line up in columns, so the numbers can be compared down the
/// table instead of read one line at a time. The provider column appears in
/// the tables that do not group by provider: there, a row can mix them.
#[allow(clippy::too_many_lines)]
pub(crate) fn telemetry_table(
    style: Style,
    title: &str,
    rows: &[(&str, &RequestSummary)],
    show_provider: bool,
) -> String {
    let mut out = format!("{}\n", style.bold(title));
    if rows.is_empty() {
        return out;
    }
    let label_width = rows
        .iter()
        .map(|(label, _)| label.chars().count())
        .max()
        .unwrap_or(0);
    let provider_width = rows
        .iter()
        .map(|(_, summary)| summary.provider_label().chars().count())
        .max()
        .unwrap_or(0);
    let busiest = rows
        .iter()
        .map(|(_, summary)| summary.token_total())
        .max()
        .unwrap_or(1)
        .max(1);
    // The header names the columns: without it, a row of numbers says
    // nothing about what it measures. The cells are built the same way the
    // rows are, so the two line up.
    let mut header = vec![
        style.faint(pad_right("", label_width)),
        style.faint(pad_right("", BAR_WIDTH)),
    ];
    if show_provider {
        header.push(style.faint(pad_right("provider", provider_width)));
    }
    header.push(style.faint(format!("{:>8}", "requests")));
    header.push(style.faint(format!("{:>8}", "in")));
    header.push(style.faint(format!("{:>8}", "out")));
    header.push(style.faint(format!("{:>8}", "reasoning")));
    header.push(style.faint(format!("{:>7}", "cache")));
    header.push(style.faint(format!("{:>9}", "ttft")));
    header.push(style.faint(format!("{:>7}", "latency")));
    header.push(style.faint("cost"));
    out.push_str(&header.join("  "));
    out.push('\n');
    for (label, summary) in rows {
        // The bar is the volume of tokens, and its colours are the cache:
        // green for the part the provider served from its cache, red for the
        // part it had to read, the rest the empty room of the busiest group.
        let volume = summary.token_total();
        let filled = usize::try_from(volume)
            .unwrap_or(0)
            .saturating_mul(BAR_WIDTH)
            / usize::try_from(busiest).unwrap_or(1);
        let filled = filled.min(BAR_WIDTH);
        let hits = if summary.input_tokens == 0 {
            0
        } else {
            usize::try_from(summary.cache_read_tokens)
                .unwrap_or(0)
                .saturating_mul(filled)
                / usize::try_from(summary.input_tokens).unwrap_or(1)
        };
        let hits = hits.min(filled);
        let bar = format!(
            "{}{}{}",
            style.green("█".repeat(hits)),
            style.red("█".repeat(filled - hits)),
            style.faint("·".repeat(BAR_WIDTH - filled)),
        );
        let mut cost = style.green(summary.cost_label());
        if summary.unpriced > 0 {
            cost = style.yellow(summary.cost_label());
        }
        let reasoning = if summary.reasoning_tokens > 0 {
            human_tokens(summary.reasoning_tokens)
        } else {
            "–".to_owned()
        };
        let cache = match summary.cache_hit_percent() {
            Some(percent) => format!("{percent}%"),
            None => "–".to_owned(),
        };
        let mut line = vec![style.cyan(pad_right(label, label_width)), bar];
        if show_provider {
            line.push(style.dim(pad_right(&summary.provider_label(), provider_width)));
        }
        line.push(style.dim(format!("{:>8}", format!("{} req", summary.requests))));
        line.push(style.dim(format!(
            "{:>8}",
            format!("{} in", human_tokens(summary.input_tokens))
        )));
        line.push(style.dim(format!(
            "{:>8}",
            format!("{} out", human_tokens(summary.output_tokens))
        )));
        line.push(style.dim(format!("{reasoning:>8}")));
        line.push(style.dim(format!("{cache:>7}")));
        line.push(style.dim(format!(
            "{:>9}",
            format!("ttft {}", duration_label(summary.average_ttft_ms()))
        )));
        line.push(style.dim(format!("{:>7}", duration_label(summary.average_ms()))));
        line.push(cost);
        out.push_str(&line.join("  "));
        out.push('\n');
    }
    out.push('\n');
    out
}

/// Pads a label to a width, counting characters and not bytes.
fn pad_right(label: &str, width: usize) -> String {
    let length = label.chars().count();
    format!("{label}{}", " ".repeat(width.saturating_sub(length)))
}

/// Moves the process into the directory named on the command line.
///
/// The directory has to exist: a typo that silently left the process where it
/// was would have the tools reading a tree nobody named, which is the failure
/// that is hardest to notice. So it is checked and reported, and the process
/// does not move.
fn enter_workdir(dir: &Path) -> Result<(), String> {
    let dir = if dir.is_absolute() {
        dir.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| format!("could not read the working directory: {error}"))?
            .join(dir)
    };
    if !dir.is_dir() {
        return Err(format!("`{}` is not a directory", dir.display()));
    }
    std::env::set_current_dir(&dir)
        .map_err(|error| format!("could not enter `{}`: {error}", dir.display()))
}

/// Which verbosity a run uses: the flag, else the config, else the default.
///
/// The order is what makes `--verbose` mean "for this run": a person who set
/// `ui.verbosity` and then typed `--verbose high` asked for high now, and one
/// who typed nothing gets what the file says. Reading it through the flag's own
/// default would make the file unreachable, which is what it was before this
/// became a function.
#[must_use]
pub(crate) fn resolve_verbosity(flag: Option<&str>, config: &Config) -> String {
    flag.map_or_else(|| config.ui.verbosity.clone(), str::to_owned)
}

pub(crate) fn load_config(path: Option<PathBuf>) -> Result<Config, String> {
    let user = path.or_else(user_config_path);
    let cwd = std::env::current_dir().map_err(|error| error.to_string())?;
    let project = find_project_config(&cwd);
    let loaded = Config::load(user.as_deref(), project.as_deref()).map_err(|e| e.to_string())?;
    for warning in &loaded.warnings {
        eprintln!(
            "warning: unknown config key `{}`",
            crate::text::terminal(warning)
        );
    }
    Ok(loaded.config)
}

/// Applies `--extension KEY=JSON` values over the loaded config.
pub(crate) fn apply_extensions(config: &mut Config, values: &[String]) -> Result<(), String> {
    for value in values {
        let Some((key, raw)) = value.split_once('=') else {
            return Err(format!("--extension expects `KEY=JSON`, got `{value}`"));
        };
        if !key.contains(':') {
            return Err(format!(
                "--extension key `{key}` must name a wire family, as in `openai.chat:provider`"
            ));
        }
        let value =
            serde_json::from_str(raw).unwrap_or_else(|_| serde_json::Value::String(raw.to_owned()));
        config.extensions.insert(key.to_owned(), value);
    }
    Ok(())
}

pub(crate) async fn load_catalogue(
    config: &Config,
    offline: bool,
    refresh: bool,
) -> Result<(Catalog, Source), String> {
    let dir = cache_dir().unwrap_or_else(|| PathBuf::from(".norte-code").join("cache"));
    let ttl = if refresh {
        Duration::ZERO
    } else {
        Duration::from_secs(config.catalog.ttl_hours.saturating_mul(60 * 60))
    };
    let loaded = load_catalog(&LoadOptions {
        url: &config.catalog.url,
        ttl,
        offline: offline || config.catalog.offline,
        cache_dir: &dir,
    })
    .await
    .map_err(|e| e.to_string())?;
    for warning in &loaded.warnings {
        eprintln!("warning: {}", crate::text::terminal(warning));
    }
    Ok((loaded.catalog, loaded.source))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_flag_beats_the_configuration_and_the_configuration_beats_nothing() {
        // The order is the whole point: `--verbose` is for one run, so it wins;
        // a run that says nothing gets the file. Reading the flag's own default
        // instead would make `ui.verbosity` unreachable, which is the bug this
        // function exists to prevent.
        let mut config = Config::default();
        config.ui.verbosity = "high".to_owned();
        assert_eq!(
            resolve_verbosity(Some("low"), &config),
            "low",
            "the flag is the run's own setting"
        );
        assert_eq!(
            resolve_verbosity(None, &config),
            "high",
            "no flag means the file decides"
        );
    }

    #[test]
    fn a_configuration_that_says_nothing_leaves_the_client_default() {
        // `medium` is what the client has always shown, and an empty file has to
        // keep behaving exactly as it did before `ui` existed.
        let config = Config::default();
        assert_eq!(resolve_verbosity(None, &config), "medium");
    }
}
