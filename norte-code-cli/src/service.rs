#![allow(clippy::all, clippy::pedantic)]
//! Service lifecycle: spawn, stop, restart, status, catalog, config.

use std::fs;
use std::io::Write;
use std::net::TcpStream;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::output;
use norte_code_protocol::catalog::catalog_dir;

/// Where the service keeps its pid file and log: `XDG_STATE_HOME`, then `HOME`.
fn state_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_STATE_HOME") {
        PathBuf::from(dir).join("norte-code")
    } else if let Some(home) = std::env::var_os("HOME") {
        PathBuf::from(home)
            .join(".local")
            .join("state")
            .join("norte-code")
    } else {
        PathBuf::from(".").join(".norte-code")
    }
}

fn pidfile_path() -> PathBuf {
    state_dir().join("server.pid")
}

fn logfile_path() -> PathBuf {
    state_dir().join("server.log")
}

pub(crate) fn service_url() -> String {
    // Prefer the cli config's service_url; fall back to the default.
    let mut url = "http://127.0.0.1:8787".to_owned();
    if let Some(path) = norte_code_protocol::config::user_cli_path()
        && let Ok(raw) = fs::read_to_string(&path)
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw)
        && let Some(s) = value.get("service_url").and_then(|v| v.as_str())
    {
        s.clone_into(&mut url);
    }
    url
}

pub(crate) fn probe(url: &str) -> bool {
    use std::io::{Read, Write};
    // Minimal HTTP GET to /raep/v1/health/live. We don't parse the body,
    // just the status line — 2xx means "alive".
    let addr = url
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .trim_end_matches('/')
        .to_owned();
    let addr = if addr.contains(':') {
        addr
    } else {
        format!("{addr}:80")
    };
    let Ok(stream) = TcpStream::connect_timeout(
        &addr
            .parse()
            .unwrap_or_else(|_| "127.0.0.1:8787".parse().unwrap()),
        Duration::from_millis(250),
    ) else {
        return false;
    };
    let mut stream = stream;
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    let request =
        format!("GET /raep/v1/health/live HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }
    let mut buf = [0u8; 1024];
    let n = stream.read(&mut buf).unwrap_or(0);
    let body = String::from_utf8_lossy(&buf[..n]);
    body.contains("\"live\"") || body.contains("\"status\":\"live\"")
}

/// Re-exec this binary in "serve" mode and return the child's PID.
///
/// The daemon is launched in a fresh session so it is not in this
/// process group and survives the terminal being closed.
fn spawn_daemon() -> Result<u32, String> {
    let exe = std::env::current_exe()
        .map_err(|e| format!("could not resolve the current executable: {e}"))?;
    let state = state_dir();
    fs::create_dir_all(&state).map_err(|e| format!("could not create {}: {e}", state.display()))?;
    let log = logfile_path();
    let log_file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log)
        .map_err(|e| format!("could not open {}: {e}", log.display()))?;

    let mut cmd = Command::new(exe);
    cmd.arg("service").arg("serve");
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::from(
        log_file.try_clone().map_err(|e| format!("{e}"))?,
    ));
    cmd.stderr(Stdio::from(log_file));
    #[cfg(unix)]
    cmd.process_group(0);

    let child = cmd
        .spawn()
        .map_err(|e| format!("failed to launch the server task: {e}"))?;
    Ok(child.id())
}

fn read_pidfile() -> Option<i32> {
    let path = pidfile_path();
    let content = fs::read_to_string(&path).ok()?;
    content.trim().parse().ok()
}

fn pid_alive(pid: i32) -> bool {
    // /proc/<pid> exists and is a directory
    std::path::Path::new(&format!("/proc/{pid}")).is_dir()
}

fn kill_pid(pid: i32, signal: i32) {
    // Use kill(2) via a tiny subprocess to avoid unsafe.
    let _ = Command::new("kill")
        .arg(format!("-{signal}"))
        .arg(pid.to_string())
        .status();
}

/// Runs the RAEP server in this process. Used by the `service serve`
/// subcommand — the daemon is launched by `service up` as a fresh
/// session via this path.
///
/// # Errors
///
/// Returns an error when the server cannot bind or start.
pub fn serve() -> Result<(), String> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("could not start the runtime: {e}"))?;
    runtime
        .block_on(async { norte_code_raep::run_server().await })
        .map_err(|e| format!("{e}"))
}

pub(crate) fn up() -> Result<(), String> {
    let url = service_url();
    if probe(&url) {
        println!(
            "  {} service already running at {}",
            output::ok("✓"),
            output::bold(&url)
        );
        return Ok(());
    }
    let log = logfile_path();
    let pid = spawn_daemon().map_err(|e| format!("{e}; check {}", log.display()))?;
    let _ = fs::write(pidfile_path(), pid.to_string());

    // Animated wait for readiness.
    let frames = output::spinner_frames();
    let start = Instant::now();
    let timeout = Duration::from_secs(20);
    let mut frame_index = 0;
    let mut ready = false;
    while start.elapsed() < timeout {
        print!(
            "\r  {} starting north-code service … {}",
            frames[frame_index % frames.len()],
            output::dim(&format!("{}s", start.elapsed().as_secs()))
        );
        let _ = std::io::stdout().flush();
        if probe(&url) {
            ready = true;
            break;
        }
        thread::sleep(Duration::from_millis(150));
        frame_index += 1;
    }
    // Clear the spinner line.
    print!("\r\x1b[2K");
    let _ = std::io::stdout().flush();

    if !ready {
        return Err(format!(
            "server did not become ready within {}s — check {}",
            timeout.as_secs(),
            log.display()
        ));
    }
    println!(
        "  {} service up — {} (pid {})",
        output::ok("✓"),
        output::bold(&url),
        pid
    );
    println!("  {} log: {}", output::muted("→"), log.display());
    Ok(())
}

pub(crate) fn down() -> Result<(), String> {
    let url = service_url();
    if !probe(&url) {
        println!("  {} service is not running", output::muted("○"));
        let _ = fs::remove_file(pidfile_path());
        return Ok(());
    }
    let pid = read_pidfile().or_else(|| find_pid_by_name("norte-code-server"));
    let Some(pid) = pid else {
        return Err("service is up but no PID could be found — kill it manually".to_owned());
    };

    kill_pid(pid, 15); // SIGTERM
    let deadline = Instant::now() + Duration::from_secs(10);
    while pid_alive(pid) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(100));
    }
    if pid_alive(pid) {
        kill_pid(pid, 9); // SIGKILL
        thread::sleep(Duration::from_millis(200));
    }
    let _ = fs::remove_file(pidfile_path());
    println!("  {} service down", output::ok("✓"));
    Ok(())
}

fn find_pid_by_name(name: &str) -> Option<i32> {
    // Linux-only fallback: scan /proc for a process whose comm matches.
    let entries = fs::read_dir("/proc").ok()?;
    for entry in entries.flatten() {
        let pid: i32 = match entry.file_name().to_string_lossy().parse() {
            Ok(p) => p,
            Err(_) => continue,
        };
        let comm = fs::read_to_string(entry.path().join("comm")).unwrap_or_default();
        if comm.trim() == name {
            return Some(pid);
        }
    }
    None
}

pub(crate) fn status() {
    let url = service_url();
    output::print_heading("norte-code service");
    output::print_kv("endpoint", &url);
    if let Some(pid) = read_pidfile().or_else(|| find_pid_by_name("norte-code-server")) {
        output::print_kv("pid", &pid.to_string());
        if let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) {
            let _ = stat; // kept for future uptime parsing
        }
    } else {
        output::print_kv("pid", output::muted("(none)").as_str());
    }

    if probe(&url) {
        output::print_kv("liveness", &output::ok("live"));
        output::print_kv("readiness", &output::ok("ready"));
        // Show agent count + default agent from a quick health probe.
        if let Some(body) = http_get(&format!("{url}/raep/v1/health/ready"))
            && let Ok(json) = serde_json::from_str::<serde_json::Value>(&body)
        {
            if let Some(agent) = json.get("agent_id").and_then(|v| v.as_str()) {
                output::print_kv("default agent", agent);
            }
            if let Some(agents) = json.get("agents").and_then(serde_json::Value::as_u64) {
                output::print_kv("agents loaded", &agents.to_string());
            }
            if let Some(replay) = json.get("replay").and_then(serde_json::Value::as_bool) {
                output::print_kv("replay", if replay { "on" } else { "off" });
            }
        }
        output::print_kv("pidfile", &pidfile_path().display().to_string());
        output::print_kv("log", &logfile_path().display().to_string());
    } else {
        output::print_kv("liveness", &output::red("offline"));
        output::print_kv("readiness", &output::red("offline"));
        output::print_kv("hint", &output::muted("run `norte service up`").clone());
    }
}

fn http_get(url: &str) -> Option<String> {
    use std::io::{Read, Write};
    let (host, _path) = if let Some(rest) = url.strip_prefix("http://") {
        let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
        (host.to_owned(), format!("/{}", path))
    } else if let Some(rest) = url.strip_prefix("https://") {
        let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
        (host.to_owned(), format!("/{}", path))
    } else {
        return None;
    };
    let host = if host.contains(':') {
        host
    } else {
        format!("{host}:80")
    };
    let mut stream =
        TcpStream::connect_timeout(&host.parse().ok()?, Duration::from_millis(500)).ok()?;
    let request_path = if url.contains("/raep/") {
        let idx = url.find("/raep/").unwrap_or(0);
        url[idx..].to_owned()
    } else {
        "/".to_owned()
    };
    let request =
        format!("GET {request_path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).ok()?;
    let mut stream = stream;
    let _ = stream.set_read_timeout(Some(Duration::from_millis(2000)));
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    loop {
        match stream.read(&mut tmp) {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
        }
    }
    let text = String::from_utf8_lossy(&buf).to_string();
    // Strip headers.
    text.split_once("\r\n\r\n")
        .map(|(_, body)| body.to_owned())
        .or_else(|| text.split_once("\n\n").map(|(_, body)| body.to_owned()))
}

pub(crate) fn catalog() -> Result<(), String> {
    output::print_heading("catalogue");
    let dir = catalog_dir().map_err(|e| format!("{e}"))?;
    let providers_path = dir.join("providers.json");
    let models_path = dir.join("models.json");
    output::print_kv("providers file", &providers_path.display().to_string());
    output::print_kv("models file", &models_path.display().to_string());
    println!();

    if !providers_path.exists() {
        println!("  {} no providers.json yet", output::muted("○"));
    } else {
        let raw = fs::read_to_string(&providers_path).map_err(|e| format!("{e}"))?;
        let providers: norte_code_wire::catalog::Providers = serde_json::from_str(&raw)
            .map_err(|e| format!("could not parse providers.json: {e}"))?;
        if providers.providers.is_empty() {
            println!("  {} no providers configured", output::muted("○"));
        } else {
            for (id, provider) in &providers.providers {
                println!(
                    "  {} {}",
                    output::accent(id),
                    output::muted(&format!("— {}", provider.name))
                );
                for wire in &provider.wires {
                    println!(
                        "       {}  {}",
                        output::dim(&wire.schema.to_string()),
                        output::muted(&wire.api)
                    );
                }
            }
        }
    }

    println!();
    if !models_path.exists() {
        println!("  {} no models.json yet", output::muted("○"));
    } else {
        let raw = fs::read_to_string(&models_path).map_err(|e| format!("{e}"))?;
        let models: norte_code_wire::catalog::Models =
            serde_json::from_str(&raw).map_err(|e| format!("could not parse models.json: {e}"))?;
        if models.models.is_empty() {
            println!("  {} no models configured", output::muted("○"));
        } else {
            // group by provider
            let mut by_provider: std::collections::BTreeMap<
                &str,
                Vec<&norte_code_wire::catalog::Model>,
            > = std::collections::BTreeMap::new();
            for model in &models.models {
                by_provider
                    .entry(&model.provider_id)
                    .or_default()
                    .push(model);
            }
            for (provider, list) in &by_provider {
                println!(
                    "  {}  {}",
                    output::accent(provider),
                    output::muted(&format!("({} models)", list.len()))
                );
                for model in list {
                    let label = if model.name.is_empty() {
                        &model.id
                    } else {
                        &model.name
                    };
                    println!("       {}  {}", output::muted("·"), label);
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn config() -> Result<(), String> {
    output::print_heading("config");
    let path = norte_code_protocol::config::user_config_path()
        .ok_or("could not resolve the user config path")?;
    if !path.exists() {
        println!(
            "  {} no service.json at {}",
            output::muted("○"),
            path.display()
        );
        return Ok(());
    }
    let raw = fs::read_to_string(&path).map_err(|e| format!("{e}"))?;
    let config: norte_code_protocol::config::ServiceConfig =
        serde_json::from_str(&raw).map_err(|e| format!("could not parse service.json: {e}"))?;
    output::print_kv("config file", &path.display().to_string());
    output::print_kv("default agent", &config.default_agent);
    output::print_kv("agents", &config.agents.len().to_string());
    output::print_heading("agents");
    for agent in &config.agents {
        println!(
            "  {} {}",
            output::accent(&agent.name),
            output::muted(&agent.description.clone().unwrap_or_default())
        );
        output::print_kv("  delegates", &agent.delegates.join(", "));
        output::print_kv(
            "  model",
            &agent
                .default_model
                .clone()
                .unwrap_or_else(|| "(inherited)".to_owned()),
        );
        output::print_kv(
            "  provider",
            &agent
                .default_provider
                .clone()
                .unwrap_or_else(|| "(inherited)".to_owned()),
        );
        output::print_kv("  reasoning", &format!("{:?}", agent.reasoning.default));
        output::print_kv("  approvals allow", &agent.approvals.allow.join(", "));
        output::print_kv("  approvals deny", &agent.approvals.deny.join(", "));
        output::print_kv("  skills enabled", &agent.skills.enabled.to_string());
        output::print_kv("  mcp servers", &agent.mcp_servers.len().to_string());
        println!();
    }
    // cli.json
    if let Some(path) = norte_code_protocol::config::user_cli_path() {
        if path.exists() {
            output::print_heading("cli");
            output::print_kv("cli file", &path.display().to_string());
            let raw = fs::read_to_string(&path).map_err(|e| format!("{e}"))?;
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) {
                if let Some(url) = value.get("service_url").and_then(|v| v.as_str()) {
                    output::print_kv("service url", url);
                }
                if let Some(agent) = value.get("agent_id").and_then(|v| v.as_str()) {
                    output::print_kv("agent id", agent);
                }
                if let Some(verbosity) = value.get("verbosity").and_then(|v| v.as_str()) {
                    output::print_kv("verbosity", verbosity);
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn logs(args: &[String]) -> Result<(), String> {
    let follow = args
        .iter()
        .any(|a| a == "--live" || a == "-f" || a == "--follow");
    output::print_heading(if follow {
        "service logs (live)"
    } else {
        "service logs"
    });
    let path = logfile_path();
    if !path.exists() {
        println!("  {} no log file at {}", output::muted("○"), path.display());
        return Ok(());
    }
    let raw = fs::read_to_string(&path).map_err(|e| format!("{e}"))?;
    let lines: Vec<&str> = raw.lines().collect();
    let tail = if lines.len() > 80 {
        &lines[lines.len() - 80..]
    } else {
        &lines
    };
    if tail.is_empty() {
        println!("  {} log is empty", output::muted("○"));
    } else {
        output::print_kv("log file", &path.display().to_string());
        output::print_kv("lines", &format!("{}", lines.len()));
        if follow {
            output::print_kv("mode", "following (Ctrl+C to stop)");
        }
        println!();
        for line in tail {
            print_log_line(line);
        }
    }
    if !follow {
        return Ok(());
    }
    let mut offset = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    loop {
        std::thread::sleep(std::time::Duration::from_millis(300));
        let new_size = match fs::metadata(&path) {
            Ok(m) => m.len(),
            Err(_) => continue,
        };
        if new_size < offset {
            offset = 0;
        }
        if new_size > offset {
            let mut file = match fs::File::open(&path) {
                Ok(f) => f,
                Err(_) => continue,
            };
            use std::io::{Read, Seek, SeekFrom};
            let _ = file.seek(SeekFrom::Start(offset));
            let mut buf = String::new();
            if file.read_to_string(&mut buf).is_ok() {
                for line in buf.lines() {
                    print_log_line(line);
                }
                offset = new_size;
            }
        }
    }
}

fn print_log_line(line: &str) {
    if line.contains("error") || line.contains("panic") || line.contains("ERROR") {
        println!("  {}", output::red(line));
    } else if line.contains("warn") || line.contains("WARN") {
        println!("  {}", output::warn(line));
    } else {
        println!("  {}", output::muted(line));
    }
}

pub(crate) fn doctor() -> Result<(), String> {
    output::print_heading("norte-code doctor");
    let mut ok_count = 0usize;
    let mut fail_count = 0usize;
    let mut warn_count = 0usize;

    macro_rules! check {
        ($label:expr, $cond:expr, $good:expr, $bad:expr) => {{
            if $cond {
                ok_count += 1;
                println!(
                    "  {}  {}  {}",
                    output::ok("✓"),
                    $label,
                    output::muted($good)
                );
            } else {
                fail_count += 1;
                println!(
                    "  {}  {}  {}",
                    output::red("✗"),
                    $label,
                    output::muted($bad)
                );
            }
        }};
    }

    // Paths
    let cfg_dir = norte_code_protocol::config::user_config_path()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()));
    output::print_heading("paths");
    if let Some(dir) = &cfg_dir {
        check!("config dir", dir.exists(), "found", "missing");
        output::print_kv("  config dir", &dir.display().to_string());
    } else {
        fail_count += 1;
        println!(
            "  {}  config dir  {}",
            output::red("✗"),
            output::muted("could not resolve (HOME?)")
        );
    }
    let state = state_dir();
    check!(
        "state dir",
        state.exists(),
        "found",
        "missing (will be created)"
    );
    output::print_kv("  state dir", &state.display().to_string());
    let data_base = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(|h| std::path::PathBuf::from(h).join(".local").join("share"))
        })
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let data_dir = data_base.join("norte-code");
    check!(
        "data dir",
        data_dir.exists(),
        "found",
        "missing (will be created)"
    );
    output::print_kv("  data dir", &data_dir.display().to_string());

    // Files
    output::print_heading("files");
    let service_path = norte_code_protocol::config::user_config_path();
    match service_path {
        Some(p) if p.exists() => {
            ok_count += 1;
            println!(
                "  {}  service.json  {}",
                output::ok("✓"),
                output::muted(&p.display().to_string())
            );
            match norte_code_protocol::config::ServiceConfig::load(Some(&p), None) {
                Ok(cfg) => {
                    ok_count += 1;
                    println!(
                        "  {}  service.json parses  ({} agents)",
                        output::ok("✓"),
                        cfg.agents.len()
                    );
                }
                Err(e) => {
                    fail_count += 1;
                    println!("  {}  service.json parses  {}", output::red("✗"), e);
                }
            }
        }
        Some(p) => {
            fail_count += 1;
            println!(
                "  {}  service.json  missing at {}",
                output::red("✗"),
                p.display()
            );
        }
        None => {
            fail_count += 1;
            println!(
                "  {}  service.json  no config path resolvable",
                output::red("✗")
            );
        }
    }
    if let Some(p) = norte_code_protocol::config::user_cli_path() {
        if p.exists() {
            ok_count += 1;
            println!(
                "  {}  cli.json  {}",
                output::ok("✓"),
                output::muted(&p.display().to_string())
            );
        } else {
            warn_count += 1;
            println!(
                "  {}  cli.json  missing at {}",
                output::warn("!"),
                p.display()
            );
        }
    }
    let cat_dir = norte_code_protocol::catalog::catalog_dir();
    match cat_dir {
        Ok(dir) => {
            let prov = dir.join("providers.json");
            let models = dir.join("models.json");
            if prov.exists() && models.exists() {
                ok_count += 1;
                println!(
                    "  {}  catalogue files  {}",
                    output::ok("✓"),
                    output::muted(&dir.display().to_string())
                );
            } else {
                warn_count += 1;
                println!(
                    "  {}  catalogue files  missing in {}",
                    output::warn("!"),
                    dir.display()
                );
            }
        }
        Err(e) => {
            fail_count += 1;
            println!("  {}  catalogue dir  {}", output::red("✗"), e);
        }
    }

    // Service
    output::print_heading("service");
    let url = service_url();
    output::print_kv("endpoint", &url);
    if probe(&url) {
        ok_count += 1;
        println!("  {}  service up", output::ok("✓"));
        if let Some(body) = http_get(&format!("{url}/raep/v1/health/ready")) {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) {
                if let Some(a) = json.get("agent_id").and_then(|v| v.as_str()) {
                    output::print_kv("  default agent", a);
                }
                if let Some(n) = json.get("agents").and_then(serde_json::Value::as_u64) {
                    output::print_kv("  agents loaded", &n.to_string());
                }
            }
        }
    } else {
        fail_count += 1;
        println!(
            "  {}  service offline  (run `norte service up`)",
            output::red("✗")
        );
    }
    if let Some(pid) = read_pidfile() {
        if pid_alive(pid) {
            ok_count += 1;
            println!("  {}  pidfile  {} (running)", output::ok("✓"), pid);
        } else {
            warn_count += 1;
            println!("  {}  pidfile  {} (stale)", output::warn("!"), pid);
        }
    } else {
        warn_count += 1;
        println!("  {}  pidfile  none", output::warn("!"));
    }

    // Summary
    output::print_heading("summary");
    println!(
        "  {} ok   {} warn   {} fail",
        output::ok(&ok_count.to_string()),
        output::warn(&warn_count.to_string()),
        output::red(&fail_count.to_string())
    );
    if fail_count > 0 {
        println!(
            "  {} fix the failures above before running `norte`",
            output::muted("→")
        );
    } else if warn_count > 0 {
        println!(
            "  {} warnings are usually harmless (missing optional files)",
            output::muted("→")
        );
    } else {
        println!("  {} everything looks good", output::ok("→"));
    }
    Ok(())
}

pub fn ensure_up() -> Result<(), String> {
    let url = service_url();
    if probe(&url) {
        return Ok(());
    }
    up()?;
    // Wait a bit more in case probe lagged.
    for _ in 0..20 {
        if probe(&url) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(format!("service did not become ready at {url}"))
}
