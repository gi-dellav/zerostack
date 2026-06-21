use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context;
use clap::Parser;
use compact_str::CompactString;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Parser, Debug)]
#[command(
    name = "opencode-to-zerostack",
    about = "Migrate OpenCode SQLite sessions into zerostack session JSON"
)]
struct Args {
    #[arg(
        long = "opencode-db",
        help = "Path to OpenCode opencode.db [default: XDG data dir/opencode/opencode.db]"
    )]
    opencode_db: Option<PathBuf>,

    #[arg(
        long = "zerostack-data-dir",
        help = "Zerostack data directory; sessions/ is created below it [default: ZS_DATA_DIR or XDG data dir/zerostack]"
    )]
    zerostack_data_dir: Option<PathBuf>,

    #[arg(
        long = "session",
        help = "Import only an OpenCode session id or id prefix; may be repeated"
    )]
    sessions: Vec<String>,

    #[arg(long = "limit", help = "Maximum number of sessions to import")]
    limit: Option<usize>,

    #[arg(
        long = "project",
        help = "Only show/import sessions whose working directory contains this text; may be repeated"
    )]
    projects: Vec<String>,

    #[arg(
        long = "days",
        help = "Only show/import sessions updated in the last N days"
    )]
    days: Option<u64>,

    #[arg(
        long = "overwrite",
        help = "Overwrite existing zerostack session files in one-shot mode"
    )]
    overwrite: bool,

    #[arg(
        long = "dry-run",
        help = "Print what would be imported without writing files"
    )]
    dry_run: bool,

    #[arg(
        long = "no-interactive",
        help = "Skip the wizard and import the matching session set directly"
    )]
    no_interactive: bool,

    #[arg(
        long = "web",
        help = "Start a small local web UI with filters and checkboxes"
    )]
    web: bool,

    #[arg(
        long = "listen",
        default_value = "127.0.0.1:8765",
        help = "Address for --web"
    )]
    listen: String,

    #[arg(long = "provider", help = "Override imported provider name")]
    provider: Option<String>,

    #[arg(long = "model", help = "Override imported model name")]
    model: Option<String>,

    #[arg(
        long = "context-window",
        default_value_t = 128_000,
        help = "Context window to store on imported sessions"
    )]
    context_window: u64,

    #[arg(
        long = "no-tools",
        default_value_t = true,
        action = clap::ArgAction::SetFalse,
        help = "Do not import OpenCode tool call/result and patch parts"
    )]
    include_tools: bool,

    #[arg(
        long = "no-reasoning",
        default_value_t = true,
        action = clap::ArgAction::SetFalse,
        help = "Do not import provider-emitted reasoning parts"
    )]
    include_reasoning: bool,

    #[arg(
        long = "ignore-dcp-compress",
        default_value_t = true,
        action = clap::ArgAction::SetFalse,
        help = "Do not use DCP compress tool calls to reconstruct effective context"
    )]
    use_dcp_compress: bool,
}

// OpenCode stores richer event parts than zerostack persists. Default mapping:
// - message.role user/assistant/system maps directly to zerostack message roles
// - text parts are concatenated as normal message content
// - file parts are kept as compact textual references
// - OpenCode compaction parts are not imported; instead, history before the
//   latest compaction is dropped because OpenCode treats that older prefix as
//   discarded
// - DCP compress tool calls are not native OpenCode compactions, but they do
//   describe effective context replacement. We reconstruct the latest DCP block
//   by expanding prior (bN) placeholders, import that as a leading summary, and
//   keep the post-compress tail. This is intentionally an effective-context
//   migration, not a perfect archival replay of every raw DB row.
// - step-start/step-finish are transient and dropped
// - tool/patch/reasoning parts are imported by default because they are often
//   the only durable record of what happened; opt out with --no-tools or
//   --no-reasoning when that extra detail is too noisy
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MessageRole {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionMessage {
    role: MessageRole,
    content: CompactString,
    estimated_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Session {
    id: CompactString,
    name: CompactString,
    messages: Vec<SessionMessage>,
    compactions: Vec<Value>,
    created_at: CompactString,
    updated_at: CompactString,
    total_input_tokens: u64,
    total_output_tokens: u64,
    total_cost: f64,
    total_estimated_tokens: u64,
    calibrated_tokens: u64,
    calibrated_msg_count: usize,
    input_token_cost: f64,
    output_token_cost: f64,
    context_window: u64,
    model: CompactString,
    provider: CompactString,
    working_dir: CompactString,
    permission_allowlist: Vec<Value>,
}

#[derive(Debug, Clone)]
struct OpenCodeSessionRow {
    id: String,
    slug: String,
    directory: String,
    title: String,
    model: Option<String>,
    time_created: i64,
    time_updated: i64,
    cost: f64,
    tokens_input: u64,
    tokens_output: u64,
}

#[derive(Debug, Clone)]
struct OpenCodeMessageRow {
    id: String,
    time_created: i64,
    data: String,
}

#[derive(Debug, Clone)]
struct OpenCodePartRow {
    data: String,
}

#[derive(Debug, Clone)]
struct ImportOptions {
    provider_override: Option<String>,
    model_override: Option<String>,
    context_window: u64,
    include_tools: bool,
    include_reasoning: bool,
    use_dcp_compress: bool,
}

#[derive(Debug, Clone)]
struct RunConfig {
    db_path: PathBuf,
    data_dir: PathBuf,
    sessions: Vec<String>,
    projects: Vec<String>,
    days: Option<u64>,
    limit: Option<usize>,
    overwrite: bool,
    dry_run: bool,
    options: ImportOptions,
}

#[derive(Debug, Clone)]
struct ProjectGroup {
    directory: String,
    count: usize,
    latest_updated: i64,
}

#[derive(Debug, Clone)]
struct CompactionCut {
    time_created: i64,
    tail_start_id: Option<String>,
    skip_message_id: Option<String>,
    summary: Option<String>,
    source: &'static str,
}

#[derive(Debug, Default)]
struct ConvertedMessages {
    messages: Vec<SessionMessage>,
    compactions: Vec<Value>,
}

#[derive(Debug, Clone)]
struct DcpBlock {
    id: String,
    summary: String,
}

#[derive(Debug, Default, PartialEq)]
struct ImportStats {
    imported: usize,
    skipped_existing: usize,
    skipped_empty: usize,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let interactive = !args.no_interactive;
    let web = args.web;
    let listen = args.listen.clone();
    let config = RunConfig {
        db_path: args.opencode_db.unwrap_or_else(default_opencode_db_path),
        data_dir: args
            .zerostack_data_dir
            .unwrap_or_else(default_zerostack_data_dir),
        sessions: args.sessions,
        projects: args.projects,
        days: args.days,
        limit: args.limit,
        overwrite: args.overwrite,
        dry_run: args.dry_run,
        options: ImportOptions {
            provider_override: args.provider,
            model_override: args.model,
            context_window: args.context_window,
            include_tools: args.include_tools,
            include_reasoning: args.include_reasoning,
            use_dcp_compress: args.use_dcp_compress,
        },
    };

    if web {
        run_web(config, &listen)
    } else if interactive {
        run_interactive(&config)
    } else {
        run_once(&config, config.overwrite)
    }
}

fn run_once(config: &RunConfig, overwrite: bool) -> anyhow::Result<()> {
    let conn = open_opencode_db(&config.db_path)?;
    let rows = load_matching_session_rows(&conn, config)?;
    if rows.is_empty() {
        println!("no OpenCode sessions matched");
        return Ok(());
    }
    let stats = import_sessions(
        &conn,
        &rows,
        &config.data_dir,
        &config.options,
        overwrite,
        config.dry_run,
    )?;
    print_import_result(&stats, &config.data_dir, config.dry_run);
    Ok(())
}

fn run_interactive(config: &RunConfig) -> anyhow::Result<()> {
    let conn = open_opencode_db(&config.db_path)?;
    let base_rows = load_session_rows(&conn, &config.sessions, None)?;
    let days = match config.days {
        Some(days) => Some(days),
        None => prompt_days()?,
    };
    let recent_rows = filter_session_rows(base_rows, &[], days, None);
    let project_filters = if config.projects.is_empty() {
        choose_projects(&recent_rows)?
    } else {
        config.projects.clone()
    };
    let rows = filter_session_rows(recent_rows, &project_filters, None, config.limit);
    if rows.is_empty() {
        println!("no OpenCode sessions matched");
        return Ok(());
    }
    println!("OpenCode to zerostack import wizard");
    println!("OpenCode DB: {}", config.db_path.display());
    println!("Zerostack data dir: {}", config.data_dir.display());
    if !project_filters.is_empty() {
        println!("Project filter: {}", project_filters.join(", "));
    }
    if let Some(days) = days {
        println!("Updated in last {days} days");
    }
    println!(
        "Select sessions to import. Enter numbers/ranges like 1,3-5; 'a' for all; empty to cancel.\n"
    );
    print_session_menu_grouped(&rows);

    let selection = prompt_line("Import selection> ")?;
    let selected = parse_selection(&selection, rows.len())?;
    if selected.is_empty() {
        println!("cancelled");
        return Ok(());
    }
    let selected_rows: Vec<OpenCodeSessionRow> = selected
        .into_iter()
        .map(|index| rows[index].clone())
        .collect();

    let overwrite = if config.dry_run {
        false
    } else if config.overwrite {
        true
    } else {
        parse_yes_no(&prompt_line(
            "Overwrite existing zerostack sessions? [y/N] ",
        )?)
    };

    let stats = import_sessions(
        &conn,
        &selected_rows,
        &config.data_dir,
        &config.options,
        overwrite,
        config.dry_run,
    )?;
    print_import_result(&stats, &config.data_dir, config.dry_run);
    Ok(())
}

fn run_web(config: RunConfig, listen: &str) -> anyhow::Result<()> {
    let listener = TcpListener::bind(listen).with_context(|| format!("binding {listen}"))?;
    let addr = listener.local_addr()?;
    println!("OpenCode to zerostack web importer: http://{addr}/");
    println!("Press Ctrl-C to stop.");
    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => {
                if let Err(err) = handle_web_connection(&config, &mut stream) {
                    eprintln!("web request failed: {err:#}");
                }
            }
            Err(err) => eprintln!("web accept failed: {err}"),
        }
    }
    Ok(())
}

fn handle_web_connection(config: &RunConfig, stream: &mut TcpStream) -> anyhow::Result<()> {
    let request = read_http_request(stream)?;
    let mut first_line = request.lines().next().unwrap_or("").split_whitespace();
    let method = first_line.next().unwrap_or("");
    let target = first_line.next().unwrap_or("/");
    let body = request
        .split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .unwrap_or("");
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let response = match (method, path) {
        ("GET", "/") => render_web_page(config, query, None)?,
        ("POST", "/import") => handle_web_import(config, body)?,
        _ => http_page("Not found", "<p>Not found.</p>"),
    };
    write_http_response(stream, &response)
}

fn read_http_request(stream: &mut TcpStream) -> anyhow::Result<String> {
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    let mut buf = Vec::new();
    let mut tmp = [0_u8; 4096];
    let mut header_end = None;
    loop {
        let n = stream.read(&mut tmp)?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
        if header_end.is_none() {
            header_end = find_bytes(&buf, b"\r\n\r\n").map(|idx| idx + 4);
        }
        if let Some(end) = header_end {
            let headers = String::from_utf8_lossy(&buf[..end]);
            let content_len = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    if name.eq_ignore_ascii_case("content-length") {
                        value.trim().parse::<usize>().ok()
                    } else {
                        None
                    }
                })
                .unwrap_or(0);
            if buf.len() >= end + content_len {
                break;
            }
        }
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn render_web_page(
    config: &RunConfig,
    query: &str,
    notice: Option<&str>,
) -> anyhow::Result<String> {
    let filtered = web_filtered_config(config, query);
    let conn = open_opencode_db(&filtered.db_path)?;
    let rows = load_matching_session_rows(&conn, &filtered)?;
    let mut body = String::new();
    if let Some(notice) = notice {
        body.push_str("<p><strong>");
        body.push_str(&html_escape(notice));
        body.push_str("</strong></p>");
    }
    body.push_str("<form method=\"get\" action=\"/\"><fieldset><legend>Filter</legend>");
    body.push_str("<label>Project contains <input name=\"project\" value=\"");
    body.push_str(&html_escape(&query_values(query, "project").join(", ")));
    body.push_str("\"></label> ");
    body.push_str("<label>Last N days <input name=\"days\" size=\"6\" value=\"");
    if let Some(days) = filtered.days {
        body.push_str(&days.to_string());
    }
    body.push_str("\"></label> ");
    body.push_str("<label>Limit <input name=\"limit\" size=\"6\" value=\"");
    if let Some(limit) = filtered.limit {
        body.push_str(&limit.to_string());
    }
    body.push_str("\"></label> <button type=\"submit\">Apply</button></fieldset></form>");

    body.push_str("<form method=\"post\" action=\"/import\">");
    body.push_str("<p><label><input type=\"checkbox\" name=\"overwrite\" value=\"1\"> overwrite existing zerostack sessions</label></p>");
    body.push_str("<p><button type=\"submit\">Import selected</button></p>");
    let mut last_dir: Option<&str> = None;
    for row in &rows {
        if last_dir != Some(row.directory.as_str()) {
            body.push_str("<h2>");
            body.push_str(&html_escape(&row.directory));
            body.push_str("</h2>");
            last_dir = Some(&row.directory);
        }
        body.push_str("<label style=\"display:block;margin:0.35em 0\"><input type=\"checkbox\" name=\"session\" value=\"");
        body.push_str(&html_escape(&row.id));
        body.push_str("\"> <code>");
        body.push_str(&html_escape(&row.id));
        body.push_str("</code> ");
        body.push_str(&html_escape(&timestamp_ms_to_rfc3339(row.time_updated)));
        body.push_str(" - ");
        body.push_str(&html_escape(session_name(row)));
        body.push_str("</label>");
    }
    if rows.is_empty() {
        body.push_str("<p>No sessions matched.</p>");
    }
    body.push_str("</form>");
    Ok(http_page("OpenCode to zerostack import", &body))
}

fn handle_web_import(config: &RunConfig, body: &str) -> anyhow::Result<String> {
    let form = parse_form_encoded(body);
    let ids: Vec<String> = form
        .iter()
        .filter(|(key, _)| key == "session")
        .map(|(_, value)| value.clone())
        .collect();
    if ids.is_empty() {
        return Ok(http_page(
            "OpenCode to zerostack import",
            "<p>No sessions selected.</p><p><a href=\"/\">Back</a></p>",
        ));
    }
    let conn = open_opencode_db(&config.db_path)?;
    let all = load_session_rows(&conn, &[], None)?;
    let rows: Vec<OpenCodeSessionRow> = ids
        .iter()
        .filter_map(|id| all.iter().find(|row| row.id == *id).cloned())
        .collect();
    let overwrite = config.overwrite || form.iter().any(|(key, _)| key == "overwrite");
    let stats = import_sessions(
        &conn,
        &rows,
        &config.data_dir,
        &config.options,
        overwrite,
        config.dry_run,
    )?;
    let mut content = String::new();
    content.push_str("<p>");
    content.push_str(&html_escape(&format!(
        "{} {} sessions ({} existing skipped, {} empty skipped)",
        if config.dry_run {
            "Would import"
        } else {
            "Imported"
        },
        stats.imported,
        stats.skipped_existing,
        stats.skipped_empty
    )));
    content.push_str("</p><p><a href=\"/\">Back</a></p>");
    Ok(http_page("OpenCode to zerostack import", &content))
}

fn web_filtered_config(config: &RunConfig, query: &str) -> RunConfig {
    let mut filtered = config.clone();
    let projects = query_values(query, "project")
        .into_iter()
        .flat_map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|part| !part.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    if !projects.is_empty() {
        filtered.projects = projects;
    }
    if let Some(days) = query_values(query, "days")
        .first()
        .and_then(|value| value.parse::<u64>().ok())
    {
        filtered.days = Some(days);
    }
    if let Some(limit) = query_values(query, "limit")
        .first()
        .and_then(|value| value.parse::<usize>().ok())
    {
        filtered.limit = Some(limit);
    }
    filtered
}

fn http_page(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>{}</title><style>body{{font-family:sans-serif;max-width:76rem;margin:2rem auto;padding:0 1rem}}code{{color:#555}}fieldset{{margin-bottom:1rem}}h2{{font-size:1.1rem;margin-top:1.4rem}}</style></head><body><h1>{}</h1>{}</body></html>",
        html_escape(title),
        html_escape(title),
        body
    )
}

fn write_http_response(stream: &mut TcpStream, body: &str) -> anyhow::Result<()> {
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    )?;
    Ok(())
}

fn query_values(query: &str, key: &str) -> Vec<String> {
    parse_form_encoded(query)
        .into_iter()
        .filter_map(|(name, value)| if name == key { Some(value) } else { None })
        .collect()
}

fn parse_form_encoded(input: &str) -> Vec<(String, String)> {
    input
        .split('&')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let (key, value) = part.split_once('=').unwrap_or((part, ""));
            (percent_decode(key), percent_decode(value))
        })
        .collect()
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                if let (Some(hi), Some(lo)) = (hex_value(bytes[i + 1]), hex_value(bytes[i + 2])) {
                    out.push((hi << 4) | lo);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn html_escape(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn choose_projects(rows: &[OpenCodeSessionRow]) -> anyhow::Result<Vec<String>> {
    let groups = project_groups(rows);
    if groups.len() <= 1 {
        return Ok(Vec::new());
    }
    println!("Projects with OpenCode sessions:\n");
    for (idx, group) in groups.iter().enumerate() {
        println!(
            "{:>3}. {:>4} sessions  latest {}  {}",
            idx + 1,
            group.count,
            timestamp_ms_to_rfc3339(group.latest_updated),
            group.directory
        );
    }
    println!("\nProject selection can be numbers/ranges, 'all', or text matched against paths.");
    let input = prompt_line("Projects [all]> ")?;
    if input.trim().is_empty()
        || input.eq_ignore_ascii_case("all")
        || input.eq_ignore_ascii_case("a")
    {
        return Ok(Vec::new());
    }
    if input
        .chars()
        .all(|ch| ch.is_ascii_digit() || matches!(ch, ',' | '-' | ' ' | '\t'))
    {
        let indexes = parse_selection(&input, groups.len())?;
        return Ok(indexes
            .into_iter()
            .map(|index| groups[index].directory.clone())
            .collect());
    }
    Ok(input
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect())
}

fn prompt_days() -> anyhow::Result<Option<u64>> {
    let input = prompt_line("Only sessions updated in the last N days [all]> ")?;
    if input.trim().is_empty() {
        return Ok(None);
    }
    let days = input
        .trim()
        .parse::<u64>()
        .with_context(|| format!("invalid day count {input:?}"))?;
    Ok(Some(days))
}

fn print_session_menu_grouped(rows: &[OpenCodeSessionRow]) {
    let mut last_dir: Option<&str> = None;
    for (idx, row) in rows.iter().enumerate() {
        if last_dir != Some(row.directory.as_str()) {
            println!("\n{}", row.directory);
            last_dir = Some(&row.directory);
        }
        println!(
            "  {:>3}. {}  {}  {}",
            idx + 1,
            row.id,
            timestamp_ms_to_rfc3339(row.time_updated),
            session_name(row),
        );
    }
}

fn prompt_line(prompt: &str) -> anyhow::Result<String> {
    print!("{prompt}");
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    Ok(line.trim().to_string())
}

fn parse_selection(input: &str, len: usize) -> anyhow::Result<Vec<usize>> {
    let input = input.trim();
    if input.is_empty() || input.eq_ignore_ascii_case("q") {
        return Ok(Vec::new());
    }
    if input.eq_ignore_ascii_case("a") || input.eq_ignore_ascii_case("all") {
        return Ok((0..len).collect());
    }
    let mut selected = Vec::new();
    for token in input.split(',') {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        if let Some((start, end)) = token.split_once('-') {
            let start = parse_selection_index(start.trim(), len)?;
            let end = parse_selection_index(end.trim(), len)?;
            let (lo, hi) = if start <= end {
                (start, end)
            } else {
                (end, start)
            };
            selected.extend(lo..=hi);
        } else {
            selected.push(parse_selection_index(token, len)?);
        }
    }
    selected.sort_unstable();
    selected.dedup();
    Ok(selected)
}

fn parse_selection_index(value: &str, len: usize) -> anyhow::Result<usize> {
    let index = value
        .parse::<usize>()
        .with_context(|| format!("invalid selection {value:?}"))?;
    if index == 0 || index > len {
        anyhow::bail!("selection {index} is outside 1..={len}");
    }
    Ok(index - 1)
}

fn parse_yes_no(input: &str) -> bool {
    matches!(input.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

fn print_import_result(stats: &ImportStats, data_dir: &Path, dry_run: bool) {
    let action = if dry_run { "would import" } else { "imported" };
    println!(
        "{action} {} sessions ({} existing skipped, {} empty skipped) into {}",
        stats.imported,
        stats.skipped_existing,
        stats.skipped_empty,
        data_dir.join("sessions").display()
    );
}

fn open_opencode_db(path: &Path) -> anyhow::Result<Connection> {
    Connection::open(path).with_context(|| format!("opening OpenCode database {}", path.display()))
}

fn import_sessions(
    conn: &Connection,
    rows: &[OpenCodeSessionRow],
    data_dir: &Path,
    options: &ImportOptions,
    overwrite: bool,
    dry_run: bool,
) -> anyhow::Result<ImportStats> {
    let session_dir = data_dir.join("sessions");
    let mut stats = ImportStats::default();
    for row in rows {
        let output_path = session_dir.join(format!("{}.json", row.id));
        if output_path.exists() && !overwrite {
            stats.skipped_existing += 1;
            eprintln!("skip existing {}", output_path.display());
            continue;
        }

        let session = convert_session(conn, row, options)?;
        if session.messages.is_empty() {
            stats.skipped_empty += 1;
            eprintln!("skip empty OpenCode session {}", row.id);
            continue;
        }

        if dry_run {
            println!(
                "would import {}: {} messages, {} -> {}",
                row.id,
                session.messages.len(),
                row.directory,
                output_path.display()
            );
        } else {
            std::fs::create_dir_all(&session_dir)?;
            let json = serde_json::to_string(&session)?;
            std::fs::write(&output_path, json)?;
            println!(
                "imported {}: {} messages -> {}",
                row.id,
                session.messages.len(),
                output_path.display()
            );
        }
        stats.imported += 1;
    }
    Ok(stats)
}

fn load_session_rows(
    conn: &Connection,
    filters: &[String],
    limit: Option<usize>,
) -> anyhow::Result<Vec<OpenCodeSessionRow>> {
    let mut stmt = conn.prepare(
        "select id, slug, directory, title, model, time_created, time_updated, cost, tokens_input, tokens_output \
         from session order by time_updated desc, id desc",
    )?;
    let filters: Vec<&str> = filters.iter().map(String::as_str).collect();
    let mut rows = Vec::new();
    let iter = stmt.query_map([], |row| {
        Ok(OpenCodeSessionRow {
            id: row.get(0)?,
            slug: row.get(1)?,
            directory: row.get(2)?,
            title: row.get(3)?,
            model: row.get(4)?,
            time_created: row.get(5)?,
            time_updated: row.get(6)?,
            cost: row.get::<_, Option<f64>>(7)?.unwrap_or(0.0),
            tokens_input: row.get::<_, Option<i64>>(8)?.unwrap_or(0).max(0) as u64,
            tokens_output: row.get::<_, Option<i64>>(9)?.unwrap_or(0).max(0) as u64,
        })
    })?;
    for row in iter {
        let row = row?;
        if !filters.is_empty() && !filters.iter().any(|filter| row.id.starts_with(filter)) {
            continue;
        }
        rows.push(row);
        if limit.is_some_and(|limit| rows.len() >= limit) {
            break;
        }
    }
    Ok(rows)
}

fn load_matching_session_rows(
    conn: &Connection,
    config: &RunConfig,
) -> anyhow::Result<Vec<OpenCodeSessionRow>> {
    let rows = load_session_rows(conn, &config.sessions, None)?;
    Ok(filter_session_rows(
        rows,
        &config.projects,
        config.days,
        config.limit,
    ))
}

fn filter_session_rows(
    rows: Vec<OpenCodeSessionRow>,
    project_filters: &[String],
    days: Option<u64>,
    limit: Option<usize>,
) -> Vec<OpenCodeSessionRow> {
    let cutoff = days.map(|days| chrono::Utc::now().timestamp_millis() - days as i64 * 86_400_000);
    let project_filters: Vec<String> = project_filters
        .iter()
        .map(|value| value.to_ascii_lowercase())
        .filter(|value| !value.trim().is_empty())
        .collect();
    rows.into_iter()
        .filter(|row| {
            project_filters.is_empty()
                || project_filters
                    .iter()
                    .any(|filter| row.directory.to_ascii_lowercase().contains(filter))
        })
        .filter(|row| cutoff.is_none_or(|cutoff| row.time_updated >= cutoff))
        .take(limit.unwrap_or(usize::MAX))
        .collect()
}

fn project_groups(rows: &[OpenCodeSessionRow]) -> Vec<ProjectGroup> {
    let mut map: BTreeMap<String, ProjectGroup> = BTreeMap::new();
    for row in rows {
        let entry = map
            .entry(row.directory.clone())
            .or_insert_with(|| ProjectGroup {
                directory: row.directory.clone(),
                count: 0,
                latest_updated: row.time_updated,
            });
        entry.count += 1;
        entry.latest_updated = entry.latest_updated.max(row.time_updated);
    }
    let mut groups: Vec<ProjectGroup> = map.into_values().collect();
    groups.sort_by(|a, b| {
        b.latest_updated
            .cmp(&a.latest_updated)
            .then_with(|| b.count.cmp(&a.count))
            .then_with(|| a.directory.cmp(&b.directory))
    });
    groups
}

fn convert_session(
    conn: &Connection,
    row: &OpenCodeSessionRow,
    options: &ImportOptions,
) -> anyhow::Result<Session> {
    let (provider, model) = resolve_provider_model(row, options);
    let converted = convert_messages(conn, &row.id, options)?;
    let total_estimated_tokens = converted.messages.iter().map(|m| m.estimated_tokens).sum();
    Ok(Session {
        id: CompactString::new(&row.id),
        name: CompactString::new(session_name(row)),
        messages: converted.messages,
        compactions: converted.compactions,
        created_at: CompactString::new(timestamp_ms_to_rfc3339(row.time_created)),
        updated_at: CompactString::new(timestamp_ms_to_rfc3339(row.time_updated)),
        total_input_tokens: row.tokens_input,
        total_output_tokens: row.tokens_output,
        total_cost: row.cost,
        total_estimated_tokens,
        calibrated_tokens: 0,
        calibrated_msg_count: 0,
        input_token_cost: 0.0,
        output_token_cost: 0.0,
        context_window: options.context_window,
        model: CompactString::new(model),
        provider: CompactString::new(provider),
        working_dir: CompactString::new(&row.directory),
        permission_allowlist: Vec::new(),
    })
}

fn convert_messages(
    conn: &Connection,
    session_id: &str,
    options: &ImportOptions,
) -> anyhow::Result<ConvertedMessages> {
    let cut = latest_compaction_cut(conn, session_id, options)?;
    let raw_messages = load_message_rows(conn, session_id)?;
    let use_tail_start = cut
        .as_ref()
        .and_then(|cut| cut.tail_start_id.as_deref())
        .is_some_and(|tail| raw_messages.iter().any(|message| message.id == tail));

    let mut messages = Vec::new();
    if let Some(summary) = cut.as_ref().and_then(|cut| cut.summary.as_deref()) {
        messages.push(SessionMessage {
            role: MessageRole::System,
            estimated_tokens: estimate_tokens(summary),
            content: CompactString::new(summary),
        });
    }

    let mut tail_started = !use_tail_start;
    let mut summarized_count = 0usize;
    for message in raw_messages {
        if let Some(cut) = &cut {
            if cut.skip_message_id.as_deref() == Some(message.id.as_str()) {
                summarized_count += 1;
                continue;
            }
            if use_tail_start {
                if !tail_started {
                    if Some(message.id.as_str()) == cut.tail_start_id.as_deref() {
                        tail_started = true;
                    } else {
                        summarized_count += 1;
                        continue;
                    }
                }
            } else if message.time_created < cut.time_created {
                summarized_count += 1;
                continue;
            }
        }

        let value = parse_json(&message.data, "message")?;
        let Some(role) = message_role(&value) else {
            continue;
        };
        let content = message_content(conn, &message.id, options)?;
        if content.trim().is_empty() {
            continue;
        }
        if is_dcp_status_content(&content) {
            continue;
        }
        messages.push(SessionMessage {
            role,
            estimated_tokens: estimate_tokens(&content),
            content: CompactString::new(content),
        });
    }

    let compactions = cut
        .as_ref()
        .and_then(|cut| cut.summary.as_ref().map(|summary| (cut, summary)))
        .map(|(cut, summary)| {
            vec![serde_json::json!({
                "summary": summary,
                "first_kept_index": 1,
                "summarized_count": summarized_count,
                "token_savings": 0,
                "created_at": timestamp_ms_to_rfc3339(cut.time_created),
                "source": cut.source,
            })]
        })
        .unwrap_or_default();

    Ok(ConvertedMessages {
        messages,
        compactions,
    })
}

fn load_message_rows(
    conn: &Connection,
    session_id: &str,
) -> anyhow::Result<Vec<OpenCodeMessageRow>> {
    let mut stmt = conn.prepare(
        "select id, time_created, data from message where session_id = ?1 order by time_created asc, id asc",
    )?;
    let rows = stmt.query_map([session_id], |row| {
        Ok(OpenCodeMessageRow {
            id: row.get(0)?,
            time_created: row.get(1)?,
            data: row.get(2)?,
        })
    })?;
    let mut messages = Vec::new();
    for row in rows {
        messages.push(row?);
    }
    Ok(messages)
}

fn latest_compaction_cut(
    conn: &Connection,
    session_id: &str,
    options: &ImportOptions,
) -> anyhow::Result<Option<CompactionCut>> {
    let native = latest_native_compaction_cut(conn, session_id)?;
    let dcp = if options.use_dcp_compress {
        latest_dcp_compress_cut(conn, session_id)?
    } else {
        None
    };
    Ok(match (native, dcp) {
        (Some(native), Some(dcp)) => {
            if dcp.time_created > native.time_created {
                Some(dcp)
            } else {
                Some(native)
            }
        }
        (native, dcp) => native.or(dcp),
    })
}

fn latest_native_compaction_cut(
    conn: &Connection,
    session_id: &str,
) -> anyhow::Result<Option<CompactionCut>> {
    let mut stmt = conn.prepare(
        "select time_created, data from part where session_id = ?1 order by time_created desc, id desc",
    )?;
    let rows = stmt.query_map([session_id], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (time_created, data) = row?;
        let value = parse_json(&data, "part")?;
        if value.get("type").and_then(Value::as_str) == Some("compaction") {
            return Ok(Some(CompactionCut {
                time_created,
                tail_start_id: value
                    .get("tail_start_id")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                skip_message_id: None,
                summary: None,
                source: "opencode-compaction",
            }));
        }
    }
    Ok(None)
}

fn latest_dcp_compress_cut(
    conn: &Connection,
    session_id: &str,
) -> anyhow::Result<Option<CompactionCut>> {
    let mut stmt = conn.prepare(
        "select time_created, message_id, data from part where session_id = ?1 order by time_created asc, id asc",
    )?;
    let rows = stmt.query_map([session_id], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;

    let mut blocks = Vec::<DcpBlock>::new();
    let mut latest = None;
    for row in rows {
        let (time_created, message_id, data) = row?;
        let value = parse_json(&data, "part")?;
        if !is_successful_dcp_compress(&value) {
            continue;
        }

        let id = format!("b{}", blocks.len() + 1);
        let summary = dcp_compress_summary(&value, &blocks);
        if summary.trim().is_empty() {
            continue;
        }

        blocks.push(DcpBlock {
            id: id.clone(),
            summary: summary.clone(),
        });
        latest = Some(CompactionCut {
            time_created,
            tail_start_id: None,
            skip_message_id: Some(message_id),
            summary: Some(summary),
            source: "dcp-compress",
        });
    }
    Ok(latest)
}

fn is_successful_dcp_compress(value: &Value) -> bool {
    value.get("type").and_then(Value::as_str) == Some("tool")
        && value.get("tool").and_then(Value::as_str) == Some("compress")
        && value
            .get("state")
            .and_then(|state| state.get("status"))
            .and_then(Value::as_str)
            == Some("completed")
}

fn dcp_compress_summary(value: &Value, blocks: &[DcpBlock]) -> String {
    let state = value.get("state").unwrap_or(&Value::Null);
    let input = state.get("input").unwrap_or(&Value::Null);
    let topic = input.get("topic").and_then(Value::as_str).unwrap_or("DCP");
    let items = input
        .get("content")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);

    let mut summaries = Vec::new();
    for item in items {
        let Some(summary) = item.get("summary").and_then(Value::as_str) else {
            continue;
        };
        let expanded = expand_dcp_placeholders(summary, blocks);
        if items.len() > 1 {
            let start = item.get("startId").and_then(Value::as_str).unwrap_or("?");
            let end = item.get("endId").and_then(Value::as_str).unwrap_or("?");
            summaries.push(format!("### {start}..{end}\n\n{}", expanded.trim()));
        } else {
            summaries.push(expanded.trim().to_string());
        }
    }

    if summaries.is_empty() {
        return String::new();
    }

    format!(
        "[Imported DCP compressed context: {topic}]\n\n{}",
        summaries.join("\n\n")
    )
}

fn expand_dcp_placeholders(summary: &str, blocks: &[DcpBlock]) -> String {
    let mut expanded = summary.to_string();
    for block in blocks {
        expanded = expanded.replace(&format!("({})", block.id), block.summary.trim());
    }
    expanded
}

fn message_content(
    conn: &Connection,
    message_id: &str,
    options: &ImportOptions,
) -> anyhow::Result<String> {
    let mut stmt = conn
        .prepare("select data from part where message_id = ?1 order by time_created asc, id asc")?;
    let rows = stmt.query_map([message_id], |row| {
        Ok(OpenCodePartRow { data: row.get(0)? })
    })?;
    let mut chunks = Vec::new();
    for row in rows {
        let row = row?;
        let value = parse_json(&row.data, "part")?;
        if let Some(text) = part_content(&value, options) {
            chunks.push(text);
        }
    }
    Ok(chunks.join("\n\n"))
}

fn part_content(value: &Value, options: &ImportOptions) -> Option<String> {
    match value.get("type")?.as_str()? {
        "text" => value.get("text")?.as_str().map(str::to_string),
        "file" => {
            let url = value.get("url").and_then(Value::as_str).unwrap_or("");
            let filename = value.get("filename").and_then(Value::as_str).unwrap_or("");
            let mime = value.get("mime").and_then(Value::as_str).unwrap_or("");
            Some(format!(
                "[file: {url}{name}{mime}]",
                name = suffix("name", filename),
                mime = suffix("mime", mime)
            ))
        }
        "reasoning" if options.include_reasoning => value
            .get("text")
            .and_then(Value::as_str)
            .map(|text| format!("[reasoning]\n{text}")),
        "tool" if options.include_tools => {
            if options.use_dcp_compress
                && value.get("tool").and_then(Value::as_str) == Some("compress")
            {
                None
            } else {
                Some(format_tool_part(value))
            }
        }
        "patch" if options.include_tools => Some(format_patch_part(value)),
        _ => None,
    }
}

fn is_dcp_status_content(content: &str) -> bool {
    let trimmed = content.trim_start();
    trimmed.starts_with("▣ DCP |") || trimmed.starts_with("▣ Compression #")
}

fn format_tool_part(value: &Value) -> String {
    let tool = value.get("tool").and_then(Value::as_str).unwrap_or("tool");
    let state = value.get("state").unwrap_or(&Value::Null);
    let status = state
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let mut out = format!("[tool: {tool} {status}]");
    if let Some(input) = state.get("input") {
        out.push_str("\ninput:\n");
        out.push_str(&json_pretty(input));
    }
    if let Some(output) = state.get("output").and_then(Value::as_str) {
        out.push_str("\noutput:\n");
        out.push_str(output);
    }
    out
}

fn format_patch_part(value: &Value) -> String {
    let hash = value.get("hash").and_then(Value::as_str).unwrap_or("");
    let files = value
        .get("files")
        .and_then(Value::as_array)
        .map(|files| {
            files
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    format!("[patch: {hash}]\n{files}")
}

fn message_role(value: &Value) -> Option<MessageRole> {
    match value.get("role")?.as_str()? {
        "user" => Some(MessageRole::User),
        "assistant" => Some(MessageRole::Assistant),
        "system" => Some(MessageRole::System),
        _ => None,
    }
}

fn resolve_provider_model(row: &OpenCodeSessionRow, options: &ImportOptions) -> (String, String) {
    let parsed = row
        .model
        .as_deref()
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok());
    let provider = options
        .provider_override
        .clone()
        .or_else(|| {
            parsed
                .as_ref()
                .and_then(|v| v.get("providerID"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| "openrouter".to_string());
    let model = options
        .model_override
        .clone()
        .or_else(|| {
            parsed
                .as_ref()
                .and_then(|v| v.get("id").or_else(|| v.get("modelID")))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| "unknown".to_string());
    (provider, model)
}

fn session_name(row: &OpenCodeSessionRow) -> &str {
    if !row.title.trim().is_empty() {
        row.title.trim()
    } else {
        row.slug.trim()
    }
}

fn parse_json(raw: &str, label: &str) -> anyhow::Result<Value> {
    serde_json::from_str(raw).with_context(|| format!("parsing OpenCode {label} JSON"))
}

fn json_pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

fn suffix(label: &str, value: &str) -> String {
    if value.is_empty() {
        String::new()
    } else {
        format!(" {label}={value}")
    }
}

fn timestamp_ms_to_rfc3339(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let millis = ms.rem_euclid(1000) as u32;
    chrono::DateTime::<chrono::Utc>::from_timestamp(secs, millis * 1_000_000)
        .unwrap_or_else(chrono::Utc::now)
        .to_rfc3339()
}

fn estimate_tokens(text: &str) -> u64 {
    let mut wide: u64 = 0;
    let mut narrow: u64 = 0;
    for ch in text.chars() {
        if is_wide_char(ch) {
            wide += 1;
        } else {
            narrow += 1;
        }
    }
    ((wide * 9 / 10) + narrow / 4).max(1)
}

fn is_wide_char(ch: char) -> bool {
    matches!(ch as u32,
        0x1100..=0x11FF |
        0x2E80..=0x9FFF |
        0xA000..=0xA4CF |
        0xAC00..=0xD7A3 |
        0xF900..=0xFAFF |
        0xFF00..=0xFFEF |
        0x20000..=0x3FFFF
    )
}

fn default_opencode_db_path() -> PathBuf {
    if let Some(dir) = std::env::var_os("OPENCODE_DATA_DIR") {
        return PathBuf::from(dir).join("opencode.db");
    }
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(dirs::data_dir)
        .unwrap_or_else(home_fallback);
    base.join("opencode").join("opencode.db")
}

fn default_zerostack_data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("ZS_DATA_DIR") {
        return PathBuf::from(dir);
    }
    let base = dirs::data_dir().unwrap_or_else(home_fallback);
    base.join("zerostack")
}

fn home_fallback() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "zerostack-opencode-import-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        path
    }

    fn test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            create table session (
                id text primary key,
                slug text not null,
                directory text not null,
                title text not null,
                model text,
                time_created integer not null,
                time_updated integer not null,
                cost real default 0 not null,
                tokens_input integer default 0 not null,
                tokens_output integer default 0 not null
            );
            create table message (
                id text primary key,
                session_id text not null,
                time_created integer not null,
                data text not null
            );
            create table part (
                id text primary key,
                message_id text not null,
                session_id text not null,
                time_created integer not null,
                data text not null
            );
            "#,
        )
        .unwrap();
        conn
    }

    fn options() -> ImportOptions {
        ImportOptions {
            provider_override: None,
            model_override: None,
            context_window: 99,
            include_tools: true,
            include_reasoning: true,
            use_dcp_compress: true,
        }
    }

    fn run_config(db_path: PathBuf, data_dir: PathBuf) -> RunConfig {
        RunConfig {
            db_path,
            data_dir,
            sessions: Vec::new(),
            projects: Vec::new(),
            days: None,
            limit: None,
            overwrite: false,
            dry_run: false,
            options: options(),
        }
    }

    fn insert_session(conn: &Connection, id: &str) {
        conn.execute(
            "insert into session values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            (
                id,
                "quiet-fox",
                "/repo",
                "Imported title",
                r#"{"id":"gpt-5.5","providerID":"openai"}"#,
                1_700_000_000_000_i64,
                1_700_000_001_000_i64,
                1.25_f64,
                123_i64,
                45_i64,
            ),
        )
        .unwrap();
    }

    fn insert_message(conn: &Connection, session_id: &str, id: &str, time: i64, role: &str) {
        conn.execute(
            "insert into message values (?1, ?2, ?3, ?4)",
            (
                id,
                session_id,
                time,
                format!(r#"{{"role":"{role}","time":{{"created":{time}}}}}"#),
            ),
        )
        .unwrap();
    }

    fn insert_part(
        conn: &Connection,
        session_id: &str,
        message_id: &str,
        id: &str,
        time: i64,
        data: &str,
    ) {
        conn.execute(
            "insert into part values (?1, ?2, ?3, ?4, ?5)",
            (id, message_id, session_id, time, data),
        )
        .unwrap();
    }

    fn insert_basic_session(conn: &Connection) {
        insert_session(conn, "ses_test");
        insert_message(conn, "ses_test", "msg_user", 1, "user");
        insert_part(
            conn,
            "ses_test",
            "msg_user",
            "part_user",
            1,
            r#"{"type":"text","text":"hello"}"#,
        );
        insert_message(conn, "ses_test", "msg_assistant", 2, "assistant");
        insert_part(
            conn,
            "ses_test",
            "msg_assistant",
            "part_assistant",
            2,
            r#"{"type":"text","text":"world"}"#,
        );
    }

    fn dcp_compress_part(topic: &str, start: &str, end: &str, summary: &str) -> String {
        serde_json::json!({
            "type": "tool",
            "tool": "compress",
            "callID": format!("call_{topic}"),
            "state": {
                "status": "completed",
                "input": {
                    "topic": topic,
                    "content": [{
                        "startId": start,
                        "endId": end,
                        "summary": summary,
                    }]
                }
            }
        })
        .to_string()
    }

    #[test]
    fn converts_basic_opencode_session_to_zerostack_schema() {
        let conn = test_db();
        insert_basic_session(&conn);
        let rows = load_session_rows(&conn, &[], None).unwrap();
        let session = convert_session(&conn, &rows[0], &options()).unwrap();
        assert_eq!(session.id, "ses_test");
        assert_eq!(session.name, "Imported title");
        assert_eq!(session.provider, "openai");
        assert_eq!(session.model, "gpt-5.5");
        assert_eq!(session.context_window, 99);
        assert_eq!(session.total_input_tokens, 123);
        assert_eq!(session.total_output_tokens, 45);
        assert_eq!(session.messages.len(), 2);
        assert_eq!(session.messages[0].role, MessageRole::User);
        assert_eq!(session.messages[0].content, "hello");
        assert_eq!(session.messages[1].role, MessageRole::Assistant);
        assert_eq!(session.messages[1].content, "world");
    }

    #[test]
    fn cuts_messages_before_latest_compaction_tail_start() {
        let conn = test_db();
        insert_session(&conn, "ses_compact");
        insert_message(&conn, "ses_compact", "msg_old", 1, "user");
        insert_part(
            &conn,
            "ses_compact",
            "msg_old",
            "part_old",
            1,
            r#"{"type":"text","text":"old"}"#,
        );
        insert_message(&conn, "ses_compact", "msg_cut", 2, "assistant");
        insert_part(
            &conn,
            "ses_compact",
            "msg_cut",
            "part_cut",
            2,
            r#"{"type":"compaction","tail_start_id":"msg_tail"}"#,
        );
        insert_message(&conn, "ses_compact", "msg_tail", 3, "user");
        insert_part(
            &conn,
            "ses_compact",
            "msg_tail",
            "part_tail",
            3,
            r#"{"type":"text","text":"tail"}"#,
        );
        let rows = load_session_rows(&conn, &["ses_compact".to_string()], None).unwrap();
        let session = convert_session(&conn, &rows[0], &options()).unwrap();
        assert_eq!(session.messages.len(), 1);
        assert_eq!(session.messages[0].content, "tail");
    }

    #[test]
    fn cuts_messages_before_latest_compaction_time_when_tail_missing() {
        let conn = test_db();
        insert_session(&conn, "ses_time_cut");
        insert_message(&conn, "ses_time_cut", "msg_old", 1, "user");
        insert_part(
            &conn,
            "ses_time_cut",
            "msg_old",
            "part_old",
            1,
            r#"{"type":"text","text":"old"}"#,
        );
        insert_message(&conn, "ses_time_cut", "msg_cut", 10, "assistant");
        insert_part(
            &conn,
            "ses_time_cut",
            "msg_cut",
            "part_cut",
            10,
            r#"{"type":"compaction","auto":true,"overflow":true}"#,
        );
        insert_message(&conn, "ses_time_cut", "msg_after", 11, "assistant");
        insert_part(
            &conn,
            "ses_time_cut",
            "msg_after",
            "part_after",
            11,
            r#"{"type":"text","text":"after"}"#,
        );
        let rows = load_session_rows(&conn, &["ses_time_cut".to_string()], None).unwrap();
        let session = convert_session(&conn, &rows[0], &options()).unwrap();
        assert_eq!(session.messages.len(), 1);
        assert_eq!(session.messages[0].content, "after");
    }

    #[test]
    fn dcp_compress_expands_blocks_and_imports_effective_tail() {
        let conn = test_db();
        insert_session(&conn, "ses_dcp");
        insert_message(&conn, "ses_dcp", "msg_old", 1, "user");
        insert_part(
            &conn,
            "ses_dcp",
            "msg_old",
            "part_old",
            1,
            r#"{"type":"text","text":"raw old text"}"#,
        );
        insert_message(&conn, "ses_dcp", "msg_compress_1", 2, "assistant");
        insert_part(
            &conn,
            "ses_dcp",
            "msg_compress_1",
            "part_compress_1",
            2,
            &dcp_compress_part("First", "m0001", "m0002", "first summary"),
        );
        insert_message(&conn, "ses_dcp", "msg_middle", 3, "assistant");
        insert_part(
            &conn,
            "ses_dcp",
            "msg_middle",
            "part_middle",
            3,
            r#"{"type":"text","text":"middle raw text"}"#,
        );
        insert_message(&conn, "ses_dcp", "msg_compress_2", 4, "assistant");
        insert_part(
            &conn,
            "ses_dcp",
            "msg_compress_2",
            "part_compress_2",
            4,
            &dcp_compress_part("Second", "b1", "m0004", "(b1)\n\nsecond summary"),
        );
        insert_message(&conn, "ses_dcp", "msg_tail", 5, "assistant");
        insert_part(
            &conn,
            "ses_dcp",
            "msg_tail",
            "part_tail",
            5,
            r#"{"type":"text","text":"tail text"}"#,
        );

        let rows = load_session_rows(&conn, &["ses_dcp".to_string()], None).unwrap();
        let session = convert_session(&conn, &rows[0], &options()).unwrap();
        assert_eq!(session.messages.len(), 2);
        assert_eq!(session.messages[0].role, MessageRole::System);
        assert!(session.messages[0].content.contains("Imported DCP"));
        assert!(session.messages[0].content.contains("first summary"));
        assert!(session.messages[0].content.contains("second summary"));
        assert!(!session.messages[0].content.contains("raw old text"));
        assert_eq!(session.messages[1].content, "tail text");
        assert_eq!(session.compactions.len(), 1);
        assert_eq!(session.compactions[0]["source"], "dcp-compress");
    }

    #[test]
    fn ignore_dcp_compress_keeps_raw_history() {
        let conn = test_db();
        insert_session(&conn, "ses_raw_dcp");
        insert_message(&conn, "ses_raw_dcp", "msg_old", 1, "user");
        insert_part(
            &conn,
            "ses_raw_dcp",
            "msg_old",
            "part_old",
            1,
            r#"{"type":"text","text":"raw old text"}"#,
        );
        insert_message(&conn, "ses_raw_dcp", "msg_compress", 2, "assistant");
        insert_part(
            &conn,
            "ses_raw_dcp",
            "msg_compress",
            "part_compress",
            2,
            &dcp_compress_part("First", "m0001", "m0002", "first summary"),
        );

        let rows = load_session_rows(&conn, &["ses_raw_dcp".to_string()], None).unwrap();
        let no_dcp = ImportOptions {
            use_dcp_compress: false,
            ..options()
        };
        let session = convert_session(&conn, &rows[0], &no_dcp).unwrap();
        assert!(
            session
                .messages
                .iter()
                .any(|msg| msg.content == "raw old text")
        );
        assert!(
            session
                .messages
                .iter()
                .any(|msg| msg.content.contains("[tool: compress completed]"))
        );
        assert!(session.compactions.is_empty());
    }

    #[test]
    fn skips_dcp_status_messages() {
        let conn = test_db();
        insert_session(&conn, "ses_dcp_status");
        insert_message(&conn, "ses_dcp_status", "msg_status", 1, "user");
        insert_part(
            &conn,
            "ses_dcp_status",
            "msg_status",
            "part_status",
            1,
            r#"{"type":"text","text":"▣ DCP | -10K removed, +1K summary\n\n▣ Compression #1"}"#,
        );
        insert_message(&conn, "ses_dcp_status", "msg_real", 2, "user");
        insert_part(
            &conn,
            "ses_dcp_status",
            "msg_real",
            "part_real",
            2,
            r#"{"type":"text","text":"real prompt"}"#,
        );

        let rows = load_session_rows(&conn, &["ses_dcp_status".to_string()], None).unwrap();
        let session = convert_session(&conn, &rows[0], &options()).unwrap();
        assert_eq!(session.messages.len(), 1);
        assert_eq!(session.messages[0].content, "real prompt");
    }

    #[test]
    fn imports_tools_and_reasoning_by_default_with_opt_out() {
        let value: Value = serde_json::from_str(
            r#"{"type":"tool","tool":"bash","state":{"status":"completed","input":{"command":"pwd"},"output":"/repo"}}"#,
        )
        .unwrap();
        let reasoning: Value =
            serde_json::from_str(r#"{"type":"reasoning","text":"thinking"}"#).unwrap();
        assert!(part_content(&value, &options()).is_some());
        assert!(
            part_content(&reasoning, &options())
                .unwrap()
                .contains("thinking")
        );

        let no_tools = ImportOptions {
            include_tools: false,
            ..options()
        };
        assert!(part_content(&value, &no_tools).is_none());

        let no_reasoning = ImportOptions {
            include_reasoning: false,
            ..options()
        };
        assert!(part_content(&reasoning, &no_reasoning).is_none());

        let text = part_content(&value, &options()).unwrap();
        assert!(text.contains("[tool: bash completed]"));
        assert!(text.contains("pwd"));
        assert!(text.contains("/repo"));
    }

    #[test]
    fn import_respects_existing_files_without_overwrite() {
        let conn = test_db();
        insert_basic_session(&conn);
        let rows = load_session_rows(&conn, &[], None).unwrap();
        let dir = temp_path("existing");
        std::fs::create_dir_all(dir.join("sessions")).unwrap();
        std::fs::write(dir.join("sessions/ses_test.json"), "old").unwrap();
        let stats = import_sessions(&conn, &rows, &dir, &options(), false, false).unwrap();
        assert_eq!(stats.skipped_existing, 1);
        assert_eq!(
            std::fs::read_to_string(dir.join("sessions/ses_test.json")).unwrap(),
            "old"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn overwrite_import_replaces_existing_files() {
        let db = temp_path("overwrite-db");
        let out = temp_path("overwrite-out");
        std::fs::create_dir_all(&db).unwrap();
        let db_path = db.join("opencode.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            r#"
            create table session (id text primary key, slug text not null, directory text not null, title text not null, model text, time_created integer not null, time_updated integer not null, cost real default 0 not null, tokens_input integer default 0 not null, tokens_output integer default 0 not null);
            create table message (id text primary key, session_id text not null, time_created integer not null, data text not null);
            create table part (id text primary key, message_id text not null, session_id text not null, time_created integer not null, data text not null);
            "#,
        )
        .unwrap();
        insert_basic_session(&conn);
        std::fs::create_dir_all(out.join("sessions")).unwrap();
        std::fs::write(out.join("sessions/ses_test.json"), "old").unwrap();
        let rows = load_session_rows(&conn, &["ses_test".to_string()], None).unwrap();
        import_sessions(&conn, &rows, &out, &options(), true, false).unwrap();
        let imported = std::fs::read_to_string(out.join("sessions/ses_test.json")).unwrap();
        assert!(imported.contains("Imported title"));
        assert_ne!(imported, "old");
        let _ = std::fs::remove_dir_all(db);
        let _ = std::fs::remove_dir_all(out);
    }

    #[test]
    fn timestamp_ms_is_rfc3339() {
        assert_eq!(
            timestamp_ms_to_rfc3339(1_700_000_000_123),
            "2023-11-14T22:13:20.123+00:00"
        );
    }

    #[test]
    fn parses_interactive_selection_numbers_ranges_and_all() {
        assert_eq!(parse_selection("1,3-4,4", 5).unwrap(), vec![0, 2, 3]);
        assert_eq!(parse_selection("4-2", 5).unwrap(), vec![1, 2, 3]);
        assert_eq!(parse_selection("all", 3).unwrap(), vec![0, 1, 2]);
        assert!(parse_selection("", 3).unwrap().is_empty());
        assert!(parse_selection("6", 5).is_err());
    }

    #[test]
    fn filters_sessions_by_project_days_and_limit() {
        let now = chrono::Utc::now().timestamp_millis();
        let rows = vec![
            OpenCodeSessionRow {
                id: "recent_repo".into(),
                slug: "a".into(),
                directory: "/home/me/repo".into(),
                title: "A".into(),
                model: None,
                time_created: now,
                time_updated: now,
                cost: 0.0,
                tokens_input: 0,
                tokens_output: 0,
            },
            OpenCodeSessionRow {
                id: "old_repo".into(),
                slug: "b".into(),
                directory: "/home/me/repo".into(),
                title: "B".into(),
                model: None,
                time_created: now - 10 * 86_400_000,
                time_updated: now - 10 * 86_400_000,
                cost: 0.0,
                tokens_input: 0,
                tokens_output: 0,
            },
            OpenCodeSessionRow {
                id: "other".into(),
                slug: "c".into(),
                directory: "/home/me/other".into(),
                title: "C".into(),
                model: None,
                time_created: now,
                time_updated: now,
                cost: 0.0,
                tokens_input: 0,
                tokens_output: 0,
            },
        ];
        let filtered = filter_session_rows(rows, &["repo".into()], Some(2), Some(5));
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].id, "recent_repo");
    }

    #[test]
    fn web_helpers_parse_query_and_render_session_checkboxes() {
        let db = temp_path("web-db");
        let out = temp_path("web-out");
        std::fs::create_dir_all(&db).unwrap();
        let db_path = db.join("opencode.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            r#"
            create table session (id text primary key, slug text not null, directory text not null, title text not null, model text, time_created integer not null, time_updated integer not null, cost real default 0 not null, tokens_input integer default 0 not null, tokens_output integer default 0 not null);
            create table message (id text primary key, session_id text not null, time_created integer not null, data text not null);
            create table part (id text primary key, message_id text not null, session_id text not null, time_created integer not null, data text not null);
            "#,
        )
        .unwrap();
        insert_basic_session(&conn);
        let config = run_config(db_path, out.clone());
        let page = render_web_page(&config, "project=repo&limit=10", None).unwrap();
        assert!(page.contains("name=\"project\""));
        assert!(page.contains("value=\"repo\""));
        assert!(page.contains("name=\"session\""));
        assert!(page.contains("ses_test"));
        assert_eq!(
            parse_form_encoded("session=ses_test&project=a+b%2Fc"),
            vec![
                ("session".to_string(), "ses_test".to_string()),
                ("project".to_string(), "a b/c".to_string())
            ]
        );
        let _ = std::fs::remove_dir_all(db);
        let _ = std::fs::remove_dir_all(out);
    }
}
