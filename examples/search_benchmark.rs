//! Live search dump for external evaluation (no scoring).
//!
//! Runs the query list, writes one schema-v1 JSON packet per case plus `manifest.json`.
//! Other tools / models judge quality from those files.
//!
//! ```bash
//! cargo run --example search_benchmark
//! cargo run --example search_benchmark -- --out target/benchmark/run1 --engine duckduckgo
//! ```

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use chrono::Utc;
use serde::Deserialize;
use serde_json::json;
use worbrow::{BrowserKind, Config, DEFAULT_ENGINE, output, search};

const DEFAULT_CASES: &str = include_str!("../tests/fixtures/benchmark_queries.json");

#[derive(Debug, Deserialize)]
struct CaseFile {
    cases: Vec<Case>,
}

#[derive(Debug, Deserialize)]
struct Case {
    id: String,
    query: String,
}

struct Args {
    out: PathBuf,
    engine: String,
    browser: BrowserKind,
    max_results: usize,
    timeout: u64,
    cases_path: Option<PathBuf>,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("search_benchmark: {e}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    let cases = load_cases(args.cases_path.as_deref())?;
    if cases.is_empty() {
        return Err("no cases in query list".into());
    }
    fs::create_dir_all(args.out.join("cases")).map_err(|e| format!("create output dir: {e}"))?;

    let started_at = Utc::now();
    let mut records = Vec::new();
    for case in &cases {
        let file = format!("cases/{}.json", sanitize_id(&case.id));
        let path = args.out.join(&file);
        eprintln!("search_benchmark: {} {:?}", case.id, case.query);
        let t0 = Instant::now();
        let config = Config::new(&case.query, &args.engine, args.browser)
            .with_max_results(args.max_results)
            .with_timeout(Duration::from_secs(args.timeout));
        let (ok, body) = match search(config) {
            Ok(outcome) => (
                true,
                output::success(&outcome.query, &outcome.results, &outcome.meta),
            ),
            Err(err) => (false, output::failure(&err)),
        };
        let elapsed_ms = t0.elapsed().as_millis() as u64;
        fs::write(&path, body).map_err(|e| format!("write {}: {e}", path.display()))?;
        records.push(json!({
            "id": case.id,
            "query": case.query,
            "ok": ok,
            "file": file,
            "elapsed_ms": elapsed_ms,
        }));
    }

    let manifest = json!({
        "kind": "search_benchmark_manifest",
        "schema_version": worbrow::SCHEMA_VERSION,
        "started_at": started_at.to_rfc3339(),
        "engine": args.engine,
        "browser": args.browser.to_string(),
        "max_results": args.max_results,
        "timeout_secs": args.timeout,
        "cases": records,
    });
    let manifest_path = args.out.join("manifest.json");
    fs::write(
        &manifest_path,
        serde_json::to_string_pretty(&manifest).expect("manifest JSON"),
    )
    .map_err(|e| format!("write manifest: {e}"))?;
    eprintln!("search_benchmark: wrote {}", manifest_path.display());
    Ok(())
}

fn load_cases(path: Option<&Path>) -> Result<Vec<Case>, String> {
    let raw = match path {
        Some(p) => fs::read_to_string(p).map_err(|e| format!("read cases {}: {e}", p.display()))?,
        None => DEFAULT_CASES.to_string(),
    };
    let parsed: CaseFile = serde_json::from_str(&raw).map_err(|e| format!("parse cases: {e}"))?;
    Ok(parsed.cases)
}

fn sanitize_id(id: &str) -> String {
    let s: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.is_empty() { "case".into() } else { s }
}

fn parse_args() -> Result<Args, String> {
    let mut out = None;
    let mut engine = DEFAULT_ENGINE.to_string();
    let mut browser = BrowserKind::Firefox;
    let mut max_results = 8usize;
    let mut timeout = 60u64;
    let mut cases_path = None;
    let mut argv = std::env::args().skip(1);
    while let Some(arg) = argv.next() {
        match arg.as_str() {
            "--out" => {
                out = Some(PathBuf::from(require_val("--out", argv.next())?));
            }
            "--engine" => engine = require_val("--engine", argv.next())?,
            "--browser" => {
                let v = require_val("--browser", argv.next())?;
                browser =
                    BrowserKind::from_arg(&v).ok_or_else(|| format!("unknown --browser {v}"))?;
            }
            "--max-results" => {
                max_results = require_val("--max-results", argv.next())?
                    .parse()
                    .map_err(|_| "invalid --max-results".to_string())?;
            }
            "--timeout" => {
                timeout = require_val("--timeout", argv.next())?
                    .parse()
                    .map_err(|_| "invalid --timeout".to_string())?;
            }
            "--cases" => {
                cases_path = Some(PathBuf::from(require_val("--cases", argv.next())?));
            }
            "--help" | "-h" => {
                eprintln!(
                    "search_benchmark [--out DIR] [--engine CHAIN] [--browser firefox|chrome|fake] [--max-results N] [--timeout SECS] [--cases PATH]"
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(Args {
        out: out.unwrap_or_else(|| {
            PathBuf::from(format!(
                "target/benchmark/{}",
                Utc::now().format("%Y%m%dT%H%M%SZ")
            ))
        }),
        engine,
        browser,
        max_results,
        timeout,
        cases_path,
    })
}

fn require_val(flag: &str, v: Option<String>) -> Result<String, String> {
    v.filter(|s| !s.is_empty())
        .ok_or_else(|| format!("{flag} requires a value"))
}
