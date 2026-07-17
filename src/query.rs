//! Stateless evidence query over the dehydrated scan layer.
//!
//! `sift query` re-runs the local scan on every invocation and filters the
//! resulting `AstSummary` evidence with flat regex flags. There is no index,
//! cache, or persisted state: the scan is fast and deterministic, so a fresh
//! scan per query can never serve stale results.

use std::io::Write;
use std::process::ExitCode;
use std::time::Instant;

use regex_lite::Regex;
use serde::Serialize;

use crate::config::{Cli, Config, OutputFormat, QueryCli, ReportLanguage};
use crate::extract::{self, AstSummary};
use crate::scanner;

const QUERY_SCHEMA_VERSION: u32 = 1;

pub fn run_query(query: QueryCli) -> ExitCode {
    let filters = match Filters::compile(&query) {
        Ok(filters) => filters,
        Err(e) => {
            eprintln!("query error: {e}");
            return ExitCode::from(2);
        }
    };
    if !filters.has_filter() {
        eprintln!(
            "query error: provide at least one filter (--calls, --imports, --signatures, --external, --any, --lang, --path)"
        );
        return ExitCode::from(2);
    }

    let cfg = match Config::resolve(scan_cli(&query)) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("query error: {e}");
            return ExitCode::from(2);
        }
    };

    let limit = query.limit;
    let started = Instant::now();
    let rx = scanner::spawn_scan(&cfg);
    let mut coverage = QueryCoverage::default();
    let mut matches: Vec<FileMatch> = Vec::new();
    let mut matched_evidence = 0usize;
    let mut shown_evidence = 0usize;

    for path in rx {
        coverage.candidate_files += 1;
        let Ok(meta) = std::fs::metadata(&path) else {
            coverage.read_failed += 1;
            continue;
        };
        if meta.len() > cfg.max_bytes {
            coverage.unsupported_files += 1;
            continue;
        }
        if extract::Lang::from_path(&path).is_none() {
            coverage.unsupported_files += 1;
            continue;
        }
        let Ok(src) = std::fs::read(&path) else {
            coverage.read_failed += 1;
            continue;
        };
        let rel = path.strip_prefix(&cfg.root).unwrap_or(&path);
        let Some(sum) = extract::dehydrate(rel, &src) else {
            coverage.parse_failed += 1;
            continue;
        };
        coverage.scanned_files += 1;
        let Some(mut file_match) = match_record(&sum, &filters) else {
            continue;
        };
        matched_evidence += file_match.evidence.len();
        // Full counts above stay honest; only the emitted evidence is capped.
        if limit > 0 {
            let room = limit.saturating_sub(shown_evidence);
            file_match.evidence.truncate(room);
        }
        shown_evidence += file_match.evidence.len();
        matches.push(file_match);
    }

    matches.sort_by(|a, b| a.path.cmp(&b.path));
    let truncated = shown_evidence < matched_evidence;
    let mut out = std::io::stdout().lock();
    let write_failed = match query.format {
        OutputFormat::Text => write_text(&mut out, &matches).is_err(),
        OutputFormat::Json => write_json(
            &mut out,
            &query,
            &coverage,
            &matches,
            matched_evidence,
            shown_evidence,
            truncated,
        )
        .is_err(),
    };
    if write_failed {
        // Broken stdout pipes from tools like head are clean exits, not crashes.
        return ExitCode::SUCCESS;
    }

    eprintln!(
        "query complete in {}ms, candidate_files: {}  scanned_files: {}  matched_files: {}  matched_evidence: {}  read_failed: {}  unsupported_files: {}  parse_failed: {}",
        started.elapsed().as_millis(),
        coverage.candidate_files,
        coverage.scanned_files,
        matches.len(),
        matched_evidence,
        coverage.read_failed,
        coverage.unsupported_files,
        coverage.parse_failed
    );
    if truncated {
        eprintln!(
            "evidence truncated: shown {shown_evidence} of {matched_evidence}; raise --limit to see more"
        );
    }

    if matches.is_empty() {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

struct Filters {
    calls: Option<Regex>,
    imports: Option<Regex>,
    signatures: Option<Regex>,
    external: Option<Regex>,
    any: Option<Regex>,
    lang: Option<String>,
    path: Option<Regex>,
}

impl Filters {
    fn compile(query: &QueryCli) -> Result<Self, String> {
        Ok(Self {
            calls: compile_one("--calls", query.calls.as_deref())?,
            imports: compile_one("--imports", query.imports.as_deref())?,
            signatures: compile_one("--signatures", query.signatures.as_deref())?,
            external: compile_one("--external", query.external.as_deref())?,
            any: compile_one("--any", query.any.as_deref())?,
            lang: query.lang.clone(),
            path: compile_one("--path", query.path.as_deref())?,
        })
    }

    fn has_filter(&self) -> bool {
        self.has_evidence_filter() || self.lang.is_some() || self.path.is_some()
    }

    fn has_evidence_filter(&self) -> bool {
        self.calls.is_some()
            || self.imports.is_some()
            || self.signatures.is_some()
            || self.external.is_some()
            || self.any.is_some()
    }
}

fn compile_one(flag: &str, pattern: Option<&str>) -> Result<Option<Regex>, String> {
    match pattern {
        None => Ok(None),
        Some(p) => Regex::new(p)
            .map(Some)
            .map_err(|e| format!("invalid regex for {flag}: {e}")),
    }
}

#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
struct Evidence {
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    line: Option<usize>,
    text: String,
}

#[derive(Debug, Serialize)]
struct FileMatch {
    path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    lang: Option<&'static str>,
    evidence: Vec<Evidence>,
}

#[derive(Debug, Default, Serialize)]
struct QueryCoverage {
    candidate_files: usize,
    scanned_files: usize,
    read_failed: usize,
    unsupported_files: usize,
    parse_failed: usize,
}

/// Record-level AND semantics: every provided evidence filter must match at
/// least once; `--lang` and `--path` further narrow which files qualify.
fn match_record(sum: &AstSummary, filters: &Filters) -> Option<FileMatch> {
    if let Some(want) = &filters.lang {
        let label = sum.lang.unwrap_or("");
        if !label.eq_ignore_ascii_case(want) {
            return None;
        }
    }
    if let Some(re) = &filters.path
        && !re.is_match(&sum.path)
    {
        return None;
    }
    if !filters.has_evidence_filter() {
        // Pure --lang/--path queries list matching files without evidence.
        return Some(FileMatch {
            path: sum.path.clone(),
            lang: sum.lang,
            evidence: Vec::new(),
        });
    }

    let mut evidence: Vec<Evidence> = Vec::new();
    let kind_filters: [(&str, &Option<Regex>); 3] = [
        ("call", &filters.calls),
        ("import", &filters.imports),
        ("signature", &filters.signatures),
    ];
    for (kind, filter) in kind_filters {
        let Some(re) = filter else { continue };
        let mut hit = false;
        for loc in &sum.locations {
            if loc.kind == kind && re.is_match(&loc.text) {
                hit = true;
                evidence.push(Evidence {
                    kind: loc.kind,
                    line: Some(loc.line),
                    text: loc.text.clone(),
                });
            }
        }
        if !hit {
            return None;
        }
    }
    if let Some(re) = &filters.external {
        let mut hit = false;
        for entry in &sum.external {
            if re.is_match(entry) {
                hit = true;
                evidence.push(Evidence {
                    kind: "external",
                    line: None,
                    text: entry.clone(),
                });
            }
        }
        if !hit {
            return None;
        }
    }
    if let Some(re) = &filters.any {
        let mut hit = false;
        for loc in &sum.locations {
            if re.is_match(&loc.text) {
                hit = true;
                evidence.push(Evidence {
                    kind: loc.kind,
                    line: Some(loc.line),
                    text: loc.text.clone(),
                });
            }
        }
        for entry in &sum.external {
            if re.is_match(entry) {
                hit = true;
                evidence.push(Evidence {
                    kind: "external",
                    line: None,
                    text: entry.clone(),
                });
            }
        }
        if !hit {
            return None;
        }
    }

    evidence.sort_by(|a, b| (a.line, a.kind, &a.text).cmp(&(b.line, b.kind, &b.text)));
    evidence.dedup();
    Some(FileMatch {
        path: sum.path.clone(),
        lang: sum.lang,
        evidence,
    })
}

fn write_text(out: &mut impl Write, matches: &[FileMatch]) -> std::io::Result<()> {
    for m in matches {
        if m.evidence.is_empty() {
            writeln!(out, "{}", m.path)?;
            continue;
        }
        for ev in &m.evidence {
            match ev.line {
                Some(line) => writeln!(out, "{}:{}: {}: {}", m.path, line, ev.kind, ev.text)?,
                None => writeln!(out, "{}:-: {}: {}", m.path, ev.kind, ev.text)?,
            }
        }
    }
    Ok(())
}

#[derive(Serialize)]
struct QueryFilterEcho<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    calls: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    imports: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    signatures: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    external: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    any: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    lang: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<&'a str>,
}

#[derive(Serialize)]
struct QueryReport<'a> {
    schema_version: u32,
    query: QueryFilterEcho<'a>,
    coverage: &'a QueryCoverage,
    matched_files: usize,
    matched_evidence: usize,
    shown_evidence: usize,
    truncated: bool,
    matches: &'a [FileMatch],
}

fn write_json(
    out: &mut impl Write,
    query: &QueryCli,
    coverage: &QueryCoverage,
    matches: &[FileMatch],
    matched_evidence: usize,
    shown_evidence: usize,
    truncated: bool,
) -> std::io::Result<()> {
    let report = QueryReport {
        schema_version: QUERY_SCHEMA_VERSION,
        query: QueryFilterEcho {
            calls: query.calls.as_deref(),
            imports: query.imports.as_deref(),
            signatures: query.signatures.as_deref(),
            external: query.external.as_deref(),
            any: query.any.as_deref(),
            lang: query.lang.as_deref(),
            path: query.path.as_deref(),
        },
        coverage,
        matched_files: matches.len(),
        matched_evidence,
        shown_evidence,
        truncated,
        matches,
    };
    match serde_json::to_string_pretty(&report) {
        Ok(json) => writeln!(out, "{json}"),
        Err(e) => {
            eprintln!("query serialization failed: {e}");
            Ok(())
        }
    }
}

/// Reuse `Config::resolve` for root/module/ignore/max-bytes resolution; the
/// query path never needs model settings, so it borrows the scan-only shape.
fn scan_cli(query: &QueryCli) -> Cli {
    Cli {
        command: None,
        target: query.target.clone(),
        module: query.module.clone(),
        api_key_file: None,
        concurrency: None,
        max_bytes: query.max_bytes,
        scan_only: true,
        agent_gate: false,
        format: OutputFormat::Text,
        benchmark: false,
        benchmark_output: None,
        benchmark_input_1m_cost: None,
        benchmark_output_1m_cost: None,
        benchmark_estimated_output_tokens: None,
        report_language: ReportLanguage::En,
        debug: query.debug,
        save: false,
        save_to: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::AstLocation;
    use std::path::PathBuf;

    fn sample_summary() -> AstSummary {
        AstSummary {
            path: "src/net.rs".to_string(),
            lang: Some("rust"),
            imports: vec!["use reqwest::Client;".to_string()],
            signatures: vec!["fn fetch_token() {".to_string()],
            calls: vec!["Command::new(\"curl\")".to_string()],
            locations: vec![
                AstLocation {
                    kind: "import",
                    line: 1,
                    text: "use reqwest::Client;".to_string(),
                },
                AstLocation {
                    kind: "signature",
                    line: 4,
                    text: "fn fetch_token() {".to_string(),
                },
                AstLocation {
                    kind: "call",
                    line: 9,
                    text: "Command::new(\"curl\")".to_string(),
                },
            ],
            external: vec!["[EXTERNAL_BLACKBOX] crate::vendor::blob".to_string()],
        }
    }

    fn filters_from(query: &QueryCli) -> Option<Filters> {
        Filters::compile(query).ok()
    }

    fn empty_query() -> QueryCli {
        QueryCli {
            target: PathBuf::from("."),
            module: None,
            calls: None,
            imports: None,
            signatures: None,
            external: None,
            any: None,
            lang: None,
            path: None,
            format: OutputFormat::Text,
            limit: 200,
            max_bytes: None,
            debug: false,
        }
    }

    #[test]
    fn calls_filter_matches_with_line_evidence() {
        let mut query = empty_query();
        query.calls = Some("curl".to_string());
        let filters = filters_from(&query);
        assert!(filters.is_some(), "filters should compile");
        let Some(filters) = filters else { return };
        let m = match_record(&sample_summary(), &filters);
        assert!(m.is_some(), "record should match");
        let Some(m) = m else { return };
        assert_eq!(m.evidence.len(), 1);
        assert_eq!(m.evidence[0].kind, "call");
        assert_eq!(m.evidence[0].line, Some(9));
    }

    #[test]
    fn record_level_and_semantics_reject_partial_match() {
        let mut query = empty_query();
        query.calls = Some("curl".to_string());
        query.imports = Some("tokio".to_string());
        let filters = filters_from(&query);
        assert!(filters.is_some(), "filters should compile");
        let Some(filters) = filters else { return };
        assert!(match_record(&sample_summary(), &filters).is_none());
    }

    #[test]
    fn any_filter_reaches_external_entries() {
        let mut query = empty_query();
        query.any = Some("EXTERNAL_BLACKBOX".to_string());
        let filters = filters_from(&query);
        assert!(filters.is_some(), "filters should compile");
        let Some(filters) = filters else { return };
        let m = match_record(&sample_summary(), &filters);
        assert!(m.is_some(), "record should match");
        let Some(m) = m else { return };
        assert_eq!(m.evidence[0].kind, "external");
        assert_eq!(m.evidence[0].line, None);
    }

    #[test]
    fn lang_and_path_narrow_records() {
        let mut query = empty_query();
        query.calls = Some("curl".to_string());
        query.lang = Some("bash".to_string());
        let filters = filters_from(&query);
        assert!(filters.is_some(), "filters should compile");
        let Some(filters) = filters else { return };
        assert!(match_record(&sample_summary(), &filters).is_none());

        let mut query = empty_query();
        query.calls = Some("curl".to_string());
        query.path = Some("^tests/".to_string());
        let filters = filters_from(&query);
        assert!(filters.is_some(), "filters should compile");
        let Some(filters) = filters else { return };
        assert!(match_record(&sample_summary(), &filters).is_none());
    }

    #[test]
    fn pure_path_query_lists_file_without_evidence() {
        let mut query = empty_query();
        query.path = Some("^src/".to_string());
        let filters = filters_from(&query);
        assert!(filters.is_some(), "filters should compile");
        let Some(filters) = filters else { return };
        let m = match_record(&sample_summary(), &filters);
        assert!(m.is_some(), "record should match");
        let Some(m) = m else { return };
        assert!(m.evidence.is_empty());
    }

    #[test]
    fn duplicate_hits_from_any_and_kind_filters_dedup() {
        let mut query = empty_query();
        query.calls = Some("curl".to_string());
        query.any = Some("curl".to_string());
        let filters = filters_from(&query);
        assert!(filters.is_some(), "filters should compile");
        let Some(filters) = filters else { return };
        let m = match_record(&sample_summary(), &filters);
        assert!(m.is_some(), "record should match");
        let Some(m) = m else { return };
        assert_eq!(m.evidence.len(), 1);
    }

    #[test]
    fn invalid_regex_is_reported_per_flag() {
        let mut query = empty_query();
        query.calls = Some("(".to_string());
        let err = Filters::compile(&query).err();
        assert!(err.is_some(), "bad regex should fail");
        let Some(err) = err else { return };
        assert!(err.contains("--calls"));
    }

    #[test]
    fn text_output_is_grep_style() {
        let mut query = empty_query();
        query.calls = Some("curl".to_string());
        let filters = filters_from(&query);
        assert!(filters.is_some(), "filters should compile");
        let Some(filters) = filters else { return };
        let m = match_record(&sample_summary(), &filters);
        assert!(m.is_some(), "record should match");
        let Some(m) = m else { return };
        let mut buf = Vec::new();
        assert!(write_text(&mut buf, &[m]).is_ok());
        let text = String::from_utf8_lossy(&buf);
        assert!(text.contains("src/net.rs:9: call: Command::new(\"curl\")"));
    }
}
