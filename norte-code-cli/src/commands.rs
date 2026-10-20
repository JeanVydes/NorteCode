//! Non-interactive subcommands and shared configuration loading.

use super::{
    BTreeMap, Catalog, Cli, Command, Config, CostArgs, CostSummary, DateTime, Duration,
    LoadOptions, PathBuf, RequestRecord, SessionDirectory, SessionsArgs, SkillRegistry, SkillsArgs,
    SkillsCommand, Source, SqliteStore, Style, TelemetryArgs, Utc, by_day, by_session, by_turn,
    cache_dir, discovery_roots, find_project_config, human_tokens, list_models, load_catalog,
    percent_label, resume_session, run_prompt, short, summarize, token_ratio, user_config_path,
};

pub(crate) async fn run(cli: Cli) -> Result<u8, String> {
    let config_path = cli.config.clone();
    let user_config = config_path.clone().or_else(user_config_path);
    let offline = cli.offline;
    let refresh = cli.refresh;
    let verbose = cli.verbose;
    let debug = cli.debug;
    let config = load_config(config_path)?;
    match cli.command {
        Command::Skills(args) => skills_command(&config, &args),
        Command::Sessions(args) => sessions_command(&args).await,
        Command::Cost(args) => cost_command(&config, &args).await,
        Command::Telemetry(args) => telemetry_command(&args).await,
        command => {
            let (catalog, source) = load_catalogue(&config, offline, refresh).await?;
            match command {
                Command::Run(args) => {
                    run_prompt(config, catalog, source, args, verbose, debug, user_config).await
                }
                Command::Resume(args) => {
                    resume_session(config, catalog, source, args, verbose, debug, user_config).await
                }
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
                        skill.name,
                        skill.root.display(),
                        skill.description
                    );
                }
            }
            Ok(0)
        }
        SkillsCommand::Check => {
            for skill in registry.skills() {
                println!(
                    "ok\t{}\t{}\t{}",
                    skill.name,
                    skill.root.display(),
                    skill.description
                );
            }
            for warning in registry.warnings() {
                eprintln!("warning: {}: {}", warning.path.display(), warning.message);
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
                session.key,
                session.title.as_deref().unwrap_or("(untitled)"),
                session.model,
                session.phase
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
            percent_label(summary.cache_read_tokens, summary.input_tokens)
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
struct RequestSummary {
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
}

impl RequestSummary {
    fn add(&mut self, record: &RequestRecord) {
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

    fn line(&mut self) -> String {
        let mut parts = vec![
            format!("{} req", self.requests),
            format!("{} in", human_tokens(self.input_tokens)),
            format!("{} out", human_tokens(self.output_tokens)),
        ];
        if self.requests > 0 && self.cache_hits > 0 {
            parts.push(format!(
                "cache {} hit",
                percent_label(self.cache_hits, self.requests)
            ));
        }
        if self.tool_calls > 0 {
            parts.push(format!("{} tool(s)", self.tool_calls));
        }
        parts.push(format!(
            "latency {} avg / {} p95",
            duration_label(self.average_ms()),
            duration_label(self.p95_ms())
        ));
        parts.push(self.cost_label());
        if self.errors + self.cancelled > 0 {
            parts.push(format!("{} error(s)", self.errors + self.cancelled));
        }
        parts.join(" · ")
    }

    #[allow(clippy::cast_precision_loss)]
    fn json(&mut self) -> serde_json::Value {
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
        "month" => at.format("%Y-%m").to_string(),
        "year" => at.format("%Y").to_string(),
        "total" => "total".to_owned(),
        _ => at.format("%Y-%m-%d").to_string(),
    }
}

fn top(groups: &BTreeMap<String, RequestSummary>) -> Vec<(&String, &RequestSummary)> {
    let mut top: Vec<_> = groups.iter().collect();
    top.sort_by(|left, right| {
        right
            .1
            .requests
            .cmp(&left.1.requests)
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
    println!("{}  {}", style.bold("total"), total.line());
    println!();
    println!("{}", style.bold(format!("by {}", args.bucket)));
    for (bucket, summary) in &mut buckets {
        println!("  {bucket:<16}  {}", summary.line());
    }
    if args.session.is_none() {
        println!();
        println!("{}", style.bold("top providers"));
        for (provider, summary) in top(&providers).into_iter().take(10) {
            let mut summary = summary.clone();
            println!("  {provider:<16}  {}", summary.line());
        }
        println!();
        println!("{}", style.bold("top models"));
        for (model, summary) in top(&models).into_iter().take(10) {
            let mut summary = summary.clone();
            println!("  {model:<40}  {}", summary.line());
        }
        println!();
        println!("{}", style.bold("sessions"));
        for (session, summary) in top(&sessions).into_iter().take(20) {
            let mut summary = summary.clone();
            println!("  {}  {}", short(session), summary.line());
        }
    }
    Ok(0)
}

pub(crate) fn load_config(path: Option<PathBuf>) -> Result<Config, String> {
    let user = path.or_else(user_config_path);
    let cwd = std::env::current_dir().map_err(|error| error.to_string())?;
    let project = find_project_config(&cwd);
    let loaded = Config::load(user.as_deref(), project.as_deref()).map_err(|e| e.to_string())?;
    for warning in &loaded.warnings {
        eprintln!("warning: unknown config key `{warning}`");
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
        eprintln!("warning: {warning}");
    }
    Ok((loaded.catalog, loaded.source))
}
