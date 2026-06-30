mod audit;
mod config;
mod extract;
mod model;
mod react;
mod report;
mod scanner;
mod skills;

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Output, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use clap::Parser;
use serde::Serialize;

use config::{Cli, CliCommand, Config, EvalCorpusCli, GithubCli, OutputFormat, ReportLanguage};

fn main() -> ExitCode {
    let run_started = Instant::now();
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("doctor")) {
        return if config::run_doctor() {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }

    if let Some(target) = internal_gate_target() {
        return run_internal_gate(target);
    }

    let mut cli = Cli::parse();
    if let Some(command) = cli.command.take() {
        return match command {
            CliCommand::Github(github) => run_github_intake(github),
            CliCommand::EvalCorpus(eval) => run_eval_corpus(eval),
        };
    }

    let cfg = match Config::resolve(cli) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("configuration error: {e}");
            return ExitCode::FAILURE;
        }
    };

    eprintln!("audit root: {}", cfg.root.display());
    eprintln!(
        "concurrency: {}  max_file_bytes: {}  scan_only: {}  agent_gate: {}  benchmark: {}  report_language: {}  debug: {}",
        cfg.concurrency,
        cfg.max_bytes,
        cfg.scan_only,
        cfg.agent_gate,
        cfg.benchmark,
        cfg.report_language.code(),
        cfg.debug
    );
    if cfg.debug {
        eprintln!("debug: ignores={}", cfg.ignores.join(","));
        eprintln!(
            "debug: legacy large endpoint={} model={} api_key_present={}",
            cfg.endpoint,
            cfg.model,
            cfg.api_key.is_some()
        );
        eprintln!(
            "debug: legacy small endpoint={} model={}",
            cfg.small_endpoint, cfg.small_model
        );
    }

    let needs_model = !(cfg.scan_only || cfg.agent_gate || cfg.benchmark);
    let needs_seed = needs_model || cfg.agent_gate || cfg.benchmark;
    let mut reg = if !needs_model {
        None
    } else {
        let r = cfg.build_registry();
        // Full audits require a large model and fail fast without prompting.
        if !r.has_large() {
            eprintln!("{}", config::missing_large_key_hint());
            return ExitCode::FAILURE;
        }
        eprintln!(
            "model layer: large={} small_pool={} degraded={}",
            r.has_large(),
            r.small.len(),
            r.degraded()
        );
        if cfg.debug {
            for line in r.debug_summaries() {
                eprintln!("debug: model {line}");
            }
        }
        Some(r)
    };

    eprintln!("scan started");
    let scan_started = Instant::now();
    let rx = scanner::spawn_scan(&cfg);
    let mut scan = ScanStats::default();
    let mut dehydrated = 0usize;
    let mut seed_records = Vec::new();
    let mut seed_candidate_bytes = 0usize;
    let mut seed_record_truncated = 0usize;
    let mut truncated_records = Vec::new();
    let mut suspicious_artifacts = Vec::new();
    const SEED_CAP: usize = 64 * 1024;
    let mut out = std::io::stdout().lock();
    for path in rx {
        scan.candidate_files += 1;
        let rel_path = audit_relative_path(&path, &cfg.root).display().to_string();
        let meta = match std::fs::metadata(&path) {
            Ok(meta) => meta,
            Err(_) => {
                scan.read_failed += 1;
                log_scan_progress(&scan, dehydrated, scan_started, cfg.debug);
                continue;
            }
        };
        if meta.len() > cfg.max_bytes {
            scan.unsupported_files += 1;
            if let Some(artifact) = inspect_suspicious_artifact(&rel_path, meta.len(), &meta, true)
            {
                suspicious_artifacts.push(artifact);
            }
            log_scan_progress(&scan, dehydrated, scan_started, cfg.debug);
            continue;
        }
        let Ok(src) = std::fs::read(&path) else {
            scan.read_failed += 1;
            log_scan_progress(&scan, dehydrated, scan_started, cfg.debug);
            continue;
        };
        if extract::Lang::from_path(&path).is_none() {
            scan.unsupported_files += 1;
            if let Some(artifact) = inspect_suspicious_artifact(&rel_path, meta.len(), &meta, false)
            {
                suspicious_artifacts.push(artifact);
            }
            log_scan_progress(&scan, dehydrated, scan_started, cfg.debug);
            continue;
        }
        // Record paths relative to the audit root so scope classification and
        // reports stay stable and never leak the host's absolute layout.
        let rel = audit_relative_path(&path, &cfg.root);
        let Some(sum) = extract::dehydrate(rel, &src) else {
            scan.parse_failed += 1;
            log_scan_progress(&scan, dehydrated, scan_started, cfg.debug);
            continue;
        };
        dehydrated += 1;
        match serde_json::to_string(&sum) {
            Ok(j) => {
                if needs_seed {
                    seed_candidate_bytes =
                        seed_candidate_bytes.saturating_add(j.len().saturating_add(1));
                    match compact_seed_record(&sum, SEED_CAP) {
                        Some(record) => {
                            if record.truncated {
                                seed_record_truncated = seed_record_truncated.saturating_add(1);
                                truncated_records.push(report::TruncatedRecord {
                                    path: record.path,
                                    original_bytes: record.original_bytes,
                                    compacted_bytes: record.json.len(),
                                    reason: record.reason,
                                });
                            }
                            seed_records.push(record.json);
                        }
                        None => scan.serialization_failed += 1,
                    }
                }
                if cfg.scan_only {
                    // Broken stdout pipes from tools like head are clean exits, not crashes.
                    if writeln!(out, "{j}").is_err() {
                        return ExitCode::SUCCESS;
                    }
                }
            }
            Err(_) => scan.serialization_failed += 1,
        }
        // ASTs are dropped inside dehydrate; full audits keep only compact JSONL records.
        log_scan_progress(&scan, dehydrated, scan_started, cfg.debug);
    }

    eprintln!(
        "scan complete, candidate_files: {}  dehydrated_files: {}  read_failed: {}  unsupported_files: {}  parse_failed: {}",
        scan.candidate_files,
        dehydrated,
        scan.read_failed,
        scan.unsupported_files,
        scan.parse_failed
    );
    let scan_elapsed_ms = elapsed_ms(scan_started);

    if reg.is_some() {
        eprintln!("preparing model seed");
    }
    let small_model_chunks_total = 0usize;
    let seed_batches = seed_batches(&seed_records, SEED_CAP);
    let seed = seed_records.join("\n");
    let seed_bytes = seed_batches
        .iter()
        .map(|batch| batch.len())
        .fold(0usize, usize::saturating_add);
    let coverage = InputCoverage {
        scan,
        dehydrated,
        seeded: seed_records.len(),
        seed_bytes,
        candidate_seed_bytes: seed_candidate_bytes,
        seed_cap: SEED_CAP,
        record_truncated: seed_record_truncated,
        truncated_records,
        suspicious_artifacts,
        batches: seed_batches.len(),
    };
    if needs_seed {
        eprintln!(
            "seed prepared, records: {}  reduce_batches: {}  seed_bytes: {}  candidate_seed_bytes: {}  record_truncated: {}",
            coverage.seeded,
            coverage.batches,
            coverage.seed_bytes,
            coverage.candidate_seed_bytes,
            coverage.record_truncated
        );
        if cfg.debug {
            let batch_bytes = seed_batches
                .iter()
                .map(|batch| batch.len().to_string())
                .collect::<Vec<_>>()
                .join(",");
            eprintln!("debug: reduce_batch_bytes=[{batch_bytes}]");
        }
    }

    if cfg.benchmark {
        let report = BenchmarkReport::from_run(
            &cfg,
            &coverage,
            scan_elapsed_ms,
            small_model_chunks_total,
            elapsed_ms(run_started),
            resident_memory_metric(),
        );
        let json = match serde_json::to_string_pretty(&report) {
            Ok(json) => json,
            Err(e) => {
                eprintln!("benchmark serialization failed: {e}");
                return ExitCode::FAILURE;
            }
        };
        if let Some(path) = &cfg.benchmark_output {
            if let Err(e) = std::fs::write(path, format!("{json}\n")) {
                eprintln!("benchmark output failed: {}: {e}", path.display());
                return ExitCode::FAILURE;
            }
            eprintln!("benchmark report: {}", path.display());
        } else {
            println!("{json}");
        }
        return ExitCode::SUCCESS;
    }

    if cfg.agent_gate {
        let gate = report::agent_gate_from_seed_with_policy(
            &seed,
            coverage.agent_gate_coverage(),
            &cfg.policy,
        );
        match cfg.format {
            OutputFormat::Text => println!("{}", gate.markdown),
            OutputFormat::Json => println!("{}", gate.json),
        }
        return if gate.safe_to_agent_run {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }

    if reg.is_some() {
        eprintln!("small-model Map skipped: reduce now converges from deterministic findings");
    }
    let react_batches = build_react_batches(&coverage, &seed_batches, cfg.report_language);
    let diagnostics = diagnostics_section(&coverage, None, cfg.report_language);

    // Drive ReACT only when a large model is configured.
    if let Some(large) = reg.as_mut().and_then(|r| r.large.as_mut()) {
        let mut final_reports = Vec::new();
        let mut partial_reports = Vec::new();
        eprintln!(
            "large-model Reduce started, batches: {}",
            react_batches.len()
        );
        for (idx, react_seed) in react_batches.iter().enumerate() {
            eprintln!(
                "large-model Reduce batch {}/{} seed_bytes={}",
                idx + 1,
                react_batches.len(),
                react_seed.len()
            );
            match react::ReAct::with_language(cfg.report_language).run(large, react_seed) {
                react::Outcome::Final(rep) => {
                    eprintln!(
                        "large-model Reduce batch {}/{} complete",
                        idx + 1,
                        react_batches.len()
                    );
                    final_reports.push(BatchReport {
                        idx,
                        bytes: react_seed.len(),
                        markdown: rep,
                    });
                }
                react::Outcome::Partial(rep) => {
                    eprintln!(
                        "partial result in Reduce batch {}/{}: {rep}",
                        idx + 1,
                        react_batches.len()
                    );
                    partial_reports.push(BatchReport {
                        idx,
                        bytes: react_seed.len(),
                        markdown: rep,
                    });
                }
            }
        }
        if partial_reports.is_empty() {
            let output = format!(
                "\n# {}\n\n{}\n\n{}\n\n## {}\n\n{}\n\n{}\n\n## {}\n\n{}",
                audit_result_heading(cfg.report_language),
                coverage.markdown_section(cfg.report_language),
                diagnostics,
                deterministic_ledger_heading(cfg.report_language),
                deterministic_ledger_note(cfg.report_language),
                report::markdown_table_from_seed_with_language(&seed, cfg.report_language),
                model_interpretation_heading(cfg.report_language),
                render_batch_reports(&final_reports, cfg.report_language)
            );
            println!("{output}");
            if cfg.save {
                save_audit_result(&cfg, &output);
            }
        } else {
            let output = format!(
                "\n# {}\n\n{}\n\n{}\n\n{}\n\n{}\n\n## {}\n\n{}",
                incomplete_audit_heading(cfg.report_language),
                incomplete_audit_notice(cfg.report_language),
                coverage.markdown_section(cfg.report_language),
                diagnostics,
                render_partial_reports(&final_reports, &partial_reports, cfg.report_language),
                local_fallback_heading(cfg.report_language),
                report::markdown_table_from_seed_with_language(&seed, cfg.report_language)
            );
            println!("{output}");
            if cfg.save {
                save_audit_result(&cfg, &output);
            }
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}

fn save_audit_result(cfg: &Config, markdown: &str) {
    let dir = match &cfg.save_to {
        Some(dir) => dir.clone(),
        None => cfg.root.join("reports"),
    };
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("cannot create save directory {}: {e}", dir.display());
        return;
    }
    let date = utc_yyyymmdd();
    let path = next_audit_result_path(&dir, &date);
    match std::fs::write(&path, markdown) {
        Ok(()) => eprintln!("audit result saved: {}", path.display()),
        Err(e) => eprintln!("cannot write audit result {}: {e}", path.display()),
    }
}

/// Pick the next free reports/sift-audit-result-<date>-<num>.md path.
fn next_audit_result_path(dir: &Path, date: &str) -> PathBuf {
    let prefix = format!("sift-audit-result-{date}-");
    let mut max_num = 0u32;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if let Some(rest) = name.strip_prefix(&prefix)
                && let Some(num) = rest.strip_suffix(".md")
                && let Ok(n) = num.parse::<u32>()
            {
                max_num = max_num.max(n);
            }
        }
    }
    let num = max_num + 1;
    dir.join(format!("sift-audit-result-{date}-{num:03}.md"))
}

/// Format the current UTC date as YYYYMMDD without external date crates.
fn utc_yyyymmdd() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}{month:02}{day:02}")
}

/// Convert days since the Unix epoch to a (year, month, day) civil date (UTC).
/// Based on Howard Hinnant's civil_from_days algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = if m <= 2 { y + 1 } else { y };
    (year, m, d)
}

fn internal_gate_target() -> Option<PathBuf> {
    let enabled = std::env::var("SIFT_INTERNAL_GATE").ok()?;
    if enabled != "1" && !enabled.eq_ignore_ascii_case("true") {
        return None;
    }
    Some(
        std::env::args_os()
            .nth(1)
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(".")),
    )
}

fn run_internal_gate(target: PathBuf) -> ExitCode {
    let project_root = match target.canonicalize() {
        Ok(root) => root,
        Err(e) => {
            eprintln!(
                "configuration error: cannot locate project root {}: {e}",
                target.display()
            );
            return ExitCode::FAILURE;
        }
    };
    eprintln!("audit root: {}", project_root.display());
    eprintln!("internal gate: true");
    match audit::write_internal_gate(&project_root) {
        Ok(result) => {
            eprintln!(
                "internal gate report: {}  FAIL: {}  WARN: {}",
                result.path.display(),
                result.failures,
                result.warnings
            );
            println!("{}", result.markdown);
            if result.failures == 0 {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(e) => {
            eprintln!("internal gate failed: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run_eval_corpus(eval: EvalCorpusCli) -> ExitCode {
    let fixtures = eval.fixtures.unwrap_or_else(|| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/repo-intake")
    });
    let cases = eval_cases();
    let mut rows = Vec::new();
    let mut failed = false;
    for case in cases {
        let fixture = fixtures.join(case.name);
        let started = Instant::now();
        let output =
            Command::new(std::env::current_exe().unwrap_or_else(|_| PathBuf::from("sift")))
                .arg(&fixture)
                .arg("--agent-gate")
                .arg("--format")
                .arg("json")
                .env_remove("SIFT_INTERNAL_GATE")
                .env_remove("SIFT_API_KEY")
                .env_remove("SIFT_SMALL_KEY")
                .output();
        let elapsed = elapsed_ms(started);
        let Ok(output) = output else {
            failed = true;
            rows.push(EvalRow::spawn_failed(case, elapsed));
            continue;
        };
        let stdout = String::from_utf8_lossy(&output.stdout);
        let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap_or_default();
        let actual_verdict = parsed
            .get("verdict")
            .and_then(|v| v.as_str())
            .unwrap_or("INVALID_JSON")
            .to_string();
        let actual_rules = eval_actual_rules(&parsed);
        let expected_rules: BTreeSet<String> =
            case.rules.iter().map(|rule| (*rule).to_string()).collect();
        let false_negative_rules = expected_rules
            .difference(&actual_rules)
            .cloned()
            .collect::<Vec<_>>();
        let false_positive_rules = actual_rules
            .difference(&expected_rules)
            .cloned()
            .collect::<Vec<_>>();
        let pass = actual_verdict == case.verdict && false_negative_rules.is_empty();
        if !pass {
            failed = true;
        }
        rows.push(EvalRow {
            fixture: case.name.to_string(),
            expected_verdict: case.verdict.to_string(),
            actual_verdict,
            expected_rules: expected_rules.into_iter().collect(),
            actual_rules: actual_rules.into_iter().collect(),
            false_negative_rules,
            false_positive_rules,
            scan_time_ms: elapsed,
            seed_bytes: parsed
                .pointer("/coverage/seed_bytes")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            estimated_tokens: parsed
                .pointer("/coverage/seed_bytes")
                .and_then(|v| v.as_u64())
                .map(|bytes| bytes.saturating_add(3) / 4)
                .unwrap_or(0),
            pass,
        });
    }
    let report = serde_json::json!({
        "schema_version": 1,
        "fixture_count": rows.len(),
        "passed": rows.iter().filter(|row| row.pass).count(),
        "failed": rows.iter().filter(|row| !row.pass).count(),
        "rows": rows,
    });
    match serde_json::to_string_pretty(&report) {
        Ok(json) => println!("{json}"),
        Err(e) => {
            eprintln!("eval-corpus serialization failed: {e}");
            return ExitCode::FAILURE;
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

#[derive(Clone, Copy)]
struct EvalCase {
    name: &'static str,
    verdict: &'static str,
    rules: &'static [&'static str],
}

#[derive(Serialize)]
struct EvalRow {
    fixture: String,
    expected_verdict: String,
    actual_verdict: String,
    expected_rules: Vec<String>,
    actual_rules: Vec<String>,
    false_negative_rules: Vec<String>,
    false_positive_rules: Vec<String>,
    scan_time_ms: u128,
    seed_bytes: u64,
    estimated_tokens: u64,
    pass: bool,
}

impl EvalRow {
    fn spawn_failed(case: EvalCase, elapsed: u128) -> Self {
        Self {
            fixture: case.name.to_string(),
            expected_verdict: case.verdict.to_string(),
            actual_verdict: "SPAWN_FAILED".to_string(),
            expected_rules: case.rules.iter().map(|rule| (*rule).to_string()).collect(),
            actual_rules: Vec::new(),
            false_negative_rules: case.rules.iter().map(|rule| (*rule).to_string()).collect(),
            false_positive_rules: Vec::new(),
            scan_time_ms: elapsed,
            seed_bytes: 0,
            estimated_tokens: 0,
            pass: false,
        }
    }
}

fn eval_cases() -> Vec<EvalCase> {
    vec![
        EvalCase {
            name: "benign-controls",
            verdict: "ACCEPT",
            rules: &[],
        },
        EvalCase {
            name: "benign-lockfiles",
            verdict: "ACCEPT",
            rules: &[],
        },
        EvalCase {
            name: "npm-postinstall-download",
            verdict: "REJECT",
            rules: &[
                "npm-lifecycle-script",
                "download-execute",
                "manifest-missing-lockfile",
            ],
        },
        EvalCase {
            name: "python-setup-command",
            verdict: "REJECT",
            rules: &["python-setup-command"],
        },
        EvalCase {
            name: "rust-build-command",
            verdict: "REJECT",
            rules: &["rust-build-script-command"],
        },
        EvalCase {
            name: "docker-curl-pipe",
            verdict: "REJECT",
            rules: &["download-execute"],
        },
        EvalCase {
            name: "makefile-hidden-network",
            verdict: "REJECT",
            rules: &["download-execute"],
        },
        EvalCase {
            name: "github-action-secret-shell",
            verdict: "CAUTION",
            rules: &["workflow-secret-shell", "unpinned-github-action"],
        },
        EvalCase {
            name: "shell-home-write",
            verdict: "REJECT",
            rules: &["install-home-write"],
        },
        EvalCase {
            name: "base64-shell",
            verdict: "REJECT",
            rules: &["base64-execute"],
        },
        EvalCase {
            name: "binary-artifact-exec",
            verdict: "REJECT",
            rules: &["download-execute"],
        },
        EvalCase {
            name: "readme-dangerous-install",
            verdict: "REJECT",
            rules: &["download-execute"],
        },
        EvalCase {
            name: "npm-git-dependency",
            verdict: "CAUTION",
            rules: &["dependency-git-source", "manifest-missing-lockfile"],
        },
        EvalCase {
            name: "cargo-git-dependency",
            verdict: "CAUTION",
            rules: &["dependency-git-source", "manifest-missing-lockfile"],
        },
        EvalCase {
            name: "python-requirements-git",
            verdict: "CAUTION",
            rules: &["dependency-git-source", "manifest-missing-lockfile"],
        },
        EvalCase {
            name: "workflow-pull-request-target",
            verdict: "REJECT",
            rules: &["workflow-pull-request-target"],
        },
        EvalCase {
            name: "workflow-write-all",
            verdict: "REJECT",
            rules: &["workflow-write-all"],
        },
        EvalCase {
            name: "docker-root-user",
            verdict: "CAUTION",
            rules: &["docker-root-user"],
        },
        EvalCase {
            name: "docker-remote-repo",
            verdict: "REJECT",
            rules: &["docker-remote-repository", "docker-default-root"],
        },
        EvalCase {
            name: "binary-extension",
            verdict: "CAUTION",
            rules: &[],
        },
        EvalCase {
            name: "archive-payload",
            verdict: "CAUTION",
            rules: &[],
        },
    ]
}

fn eval_actual_rules(parsed: &serde_json::Value) -> BTreeSet<String> {
    let mut rules = BTreeSet::new();
    if let Some(findings) = parsed.get("findings").and_then(|v| v.as_array()) {
        for finding in findings {
            if let Some(rule) = finding.get("rule").and_then(|v| v.as_str()) {
                rules.insert(rule.to_string());
            }
        }
    }
    rules
}

struct GithubSource {
    repo: String,
    url: String,
    requested_ref: String,
    resolved_commit: String,
    file_count: usize,
    byte_size: u64,
    has_submodules: bool,
    has_lfs: bool,
    checkout_path: PathBuf,
    cleanup: &'static str,
}

struct GithubRepo {
    owner: String,
    repo: String,
    url: String,
}

struct CheckoutInspection {
    file_count: usize,
    byte_size: u64,
    has_submodules: bool,
    has_lfs: bool,
}

fn run_github_intake(github: GithubCli) -> ExitCode {
    let repo = match parse_github_repo(&github.repo) {
        Ok(repo) => repo,
        Err(e) => {
            eprintln!("github intake error: unsupported_url: {e}");
            return ExitCode::FAILURE;
        }
    };
    let requested_ref = github
        .ref_name
        .clone()
        .unwrap_or_else(|| "HEAD".to_string());
    if !valid_git_ref_arg(&requested_ref) {
        eprintln!("github intake error: invalid_ref: invalid --ref value");
        return ExitCode::FAILURE;
    }
    let modes = [github.scan_only, github.agent_gate, github.benchmark]
        .iter()
        .filter(|enabled| **enabled)
        .count();
    if modes > 1 {
        eprintln!(
            "github intake error: --scan-only, --agent-gate, and --benchmark cannot be combined"
        );
        return ExitCode::FAILURE;
    }

    let temp_root = temp_checkout_root(&repo.owner, &repo.repo);
    let checkout = temp_root.join("repo");
    if let Err(e) = std::fs::create_dir_all(&checkout) {
        eprintln!(
            "github intake error: cannot create temporary checkout {}: {e}",
            checkout.display()
        );
        return ExitCode::FAILURE;
    }

    eprintln!("github source: {}", repo.url);
    eprintln!("github requested_ref: {requested_ref}");
    eprintln!("github checkout: {}", checkout.display());
    eprintln!(
        "github safety: no build, no install, no repository commands executed (no_build={} no_install={})",
        github.no_build, github.no_install
    );

    let fetch_result = fetch_github_repo(&repo.url, &checkout, &requested_ref);
    let resolved_commit = match fetch_result {
        Ok(commit) => commit,
        Err(e) => {
            eprintln!("github intake error: {e}");
            cleanup_checkout(&temp_root, github.keep_checkout);
            return ExitCode::FAILURE;
        }
    };
    eprintln!("github resolved_commit: {resolved_commit}");
    let inspection = match inspect_checkout(
        &checkout,
        github.max_checkout_files,
        github.max_checkout_bytes,
    ) {
        Ok(inspection) => inspection,
        Err(e) => {
            eprintln!("github intake error: {e}");
            cleanup_checkout(&temp_root, github.keep_checkout);
            return ExitCode::FAILURE;
        }
    };
    eprintln!(
        "github checkout_limits: files={} bytes={} submodules={} lfs={}",
        inspection.file_count, inspection.byte_size, inspection.has_submodules, inspection.has_lfs
    );

    let source = GithubSource {
        repo: format!("{}/{}", repo.owner, repo.repo),
        url: repo.url,
        requested_ref,
        resolved_commit,
        file_count: inspection.file_count,
        byte_size: inspection.byte_size,
        has_submodules: inspection.has_submodules,
        has_lfs: inspection.has_lfs,
        checkout_path: checkout.clone(),
        cleanup: if github.keep_checkout {
            "preserved"
        } else {
            "removed"
        },
    };
    let output = match run_local_sift_for_github(&github, &checkout) {
        Ok(output) => output,
        Err(e) => {
            eprintln!("github intake error: {e}");
            cleanup_checkout(&temp_root, github.keep_checkout);
            return ExitCode::FAILURE;
        }
    };

    if !output.stderr.is_empty() {
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    if github.benchmark {
        match benchmark_with_github_source(&stdout, &source) {
            Ok(json) => {
                if let Some(path) = &github.benchmark_output {
                    if let Err(e) = std::fs::write(path, format!("{json}\n")) {
                        eprintln!("benchmark output failed: {}: {e}", path.display());
                        cleanup_checkout(&temp_root, github.keep_checkout);
                        return ExitCode::FAILURE;
                    }
                    eprintln!("benchmark report: {}", path.display());
                } else {
                    println!("{json}");
                }
            }
            Err(e) => {
                eprintln!("github intake error: cannot annotate benchmark JSON: {e}");
                print!("{stdout}");
            }
        }
    } else {
        if !github.scan_only && github.format == OutputFormat::Json {
            match json_with_github_source(&stdout, &source) {
                Ok(json) => println!("{json}"),
                Err(e) => {
                    eprintln!("github intake error: cannot annotate gate JSON: {e}");
                    print!("{stdout}");
                }
            }
        } else {
            print!("{stdout}");
        }
        if !github.scan_only && github.format == OutputFormat::Text {
            print!("{}", github_source_block(&source));
        }
    }

    cleanup_checkout(&temp_root, github.keep_checkout);
    exit_code_from_output(&output)
}

fn parse_github_repo(input: &str) -> Result<GithubRepo, String> {
    if input.contains('@') {
        return Err("authenticated GitHub URLs are not accepted; pass a public owner/repo or HTTPS URL without credentials".to_string());
    }
    let trimmed = input.trim();
    let path = if let Some(rest) = trimmed.strip_prefix("https://github.com/") {
        rest
    } else if trimmed.contains("://") {
        return Err("only https://github.com/owner/repo or owner/repo are supported".to_string());
    } else {
        trimmed
    };
    let path = path.trim_matches('/').trim_end_matches(".git");
    if path.contains('?') || path.contains('#') {
        return Err("GitHub URLs with query strings or fragments are not supported".to_string());
    }
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() != 2 || !valid_github_segment(parts[0]) || !valid_github_segment(parts[1]) {
        return Err("expected GitHub repository as owner/repo".to_string());
    }
    Ok(GithubRepo {
        owner: parts[0].to_string(),
        repo: parts[1].to_string(),
        url: format!("https://github.com/{}/{}.git", parts[0], parts[1]),
    })
}

fn valid_github_segment(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

fn valid_git_ref_arg(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('-')
        && !value.chars().any(|c| c.is_control() || c.is_whitespace())
        && !value
            .chars()
            .any(|c| matches!(c, ':' | '\\' | '~' | '^' | '?' | '*' | '['))
        && !value.contains("..")
        && !value.contains("@{")
        && !value.ends_with(".lock")
}

fn temp_checkout_root(owner: &str, repo: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!(
        "sift-github-{owner}-{repo}-{}-{nanos}",
        std::process::id()
    ))
}

fn fetch_github_repo(url: &str, checkout: &Path, requested_ref: &str) -> Result<String, String> {
    run_git(&["init", "--quiet", path_arg(checkout).as_str()])?;
    run_git(&[
        "-C",
        path_arg(checkout).as_str(),
        "config",
        "core.hooksPath",
        "/dev/null",
    ])?;
    run_git(&[
        "-C",
        path_arg(checkout).as_str(),
        "remote",
        "add",
        "origin",
        url,
    ])?;
    run_git(&[
        "-C",
        path_arg(checkout).as_str(),
        "fetch",
        "--quiet",
        "--depth",
        "1",
        "--no-tags",
        "origin",
        requested_ref,
    ])?;
    run_git(&[
        "-C",
        path_arg(checkout).as_str(),
        "-c",
        "advice.detachedHead=false",
        "checkout",
        "--quiet",
        "--detach",
        "FETCH_HEAD",
    ])?;
    let output = run_git(&["-C", path_arg(checkout).as_str(), "rev-parse", "HEAD"])?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn inspect_checkout(
    checkout: &Path,
    max_files: usize,
    max_bytes: u64,
) -> Result<CheckoutInspection, String> {
    let mut inspection = CheckoutInspection {
        file_count: 0,
        byte_size: 0,
        has_submodules: checkout.join(".gitmodules").is_file(),
        has_lfs: false,
    };
    inspect_checkout_dir(checkout, checkout, max_files, max_bytes, &mut inspection)?;
    Ok(inspection)
}

fn inspect_checkout_dir(
    root: &Path,
    dir: &Path,
    max_files: usize,
    max_bytes: u64,
    inspection: &mut CheckoutInspection,
) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| {
        format!(
            "partial_scan: cannot read checkout directory {}: {e}",
            dir.display()
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|e| {
            format!(
                "partial_scan: cannot read checkout entry {}: {e}",
                dir.display()
            )
        })?;
        let path = entry.path();
        let name = entry.file_name();
        if name.to_str() == Some(".git") {
            continue;
        }
        let meta = entry.metadata().map_err(|e| {
            format!(
                "partial_scan: cannot stat checkout path {}: {e}",
                path.display()
            )
        })?;
        if meta.is_dir() {
            inspect_checkout_dir(root, &path, max_files, max_bytes, inspection)?;
            continue;
        }
        if !meta.is_file() {
            continue;
        }
        inspection.file_count = inspection.file_count.saturating_add(1);
        inspection.byte_size = inspection.byte_size.saturating_add(meta.len());
        if path.file_name().and_then(|n| n.to_str()) == Some(".gitattributes")
            && std::fs::read_to_string(&path)
                .map(|src| src.contains("filter=lfs") || src.contains("filter lfs"))
                .unwrap_or(false)
        {
            inspection.has_lfs = true;
        }
        if inspection.file_count > max_files {
            return Err(format!(
                "oversized_repo: file_count={} exceeds max_checkout_files={max_files}",
                inspection.file_count
            ));
        }
        if inspection.byte_size > max_bytes {
            return Err(format!(
                "oversized_repo: byte_size={} exceeds max_checkout_bytes={max_bytes}",
                inspection.byte_size
            ));
        }
    }
    let _ = root;
    Ok(())
}

fn path_arg(path: &Path) -> String {
    path.display().to_string()
}

fn run_git(args: &[&str]) -> Result<Output, String> {
    let mut command = Command::new("git");
    command
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0");
    let output = run_command_with_timeout(command, Duration::from_secs(120))?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(format!(
            "checkout_failure: git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

fn run_local_sift_for_github(github: &GithubCli, checkout: &Path) -> Result<Output, String> {
    let exe =
        std::env::current_exe().map_err(|e| format!("cannot locate current executable: {e}"))?;
    let mut args = vec![path_arg(checkout)];
    if let Some(module) = &github.module {
        args.push("--module".to_string());
        args.push(path_arg(module));
    }
    if github.scan_only {
        args.push("--scan-only".to_string());
    } else if github.benchmark {
        args.push("--benchmark".to_string());
    } else {
        args.push("--agent-gate".to_string());
    }
    if github.agent_gate || (!github.scan_only && !github.benchmark) {
        args.push("--format".to_string());
        args.push(match github.format {
            OutputFormat::Text => "text".to_string(),
            OutputFormat::Json => "json".to_string(),
        });
    }
    if let Some(value) = github.benchmark_input_1m_cost {
        args.push("--benchmark-input-1m-cost".to_string());
        args.push(value.to_string());
    }
    if let Some(value) = github.benchmark_output_1m_cost {
        args.push("--benchmark-output-1m-cost".to_string());
        args.push(value.to_string());
    }
    if let Some(value) = github.benchmark_estimated_output_tokens {
        args.push("--benchmark-estimated-output-tokens".to_string());
        args.push(value.to_string());
    }
    args.push("--report-language".to_string());
    args.push(github.report_language.code().to_string());
    if github.debug {
        args.push("--debug".to_string());
    }

    let mut command = Command::new(exe);
    command.args(args).env_remove("SIFT_INTERNAL_GATE");
    run_command_with_timeout(command, Duration::from_secs(600))
}

fn run_command_with_timeout(mut command: Command, timeout: Duration) -> Result<Output, String> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|e| format!("cannot spawn command: {e}"))?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                return child
                    .wait_with_output()
                    .map_err(|e| format!("cannot collect command output: {e}"));
            }
            Ok(None) => {
                if started.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("command timed out after {}s", timeout.as_secs()));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("cannot poll command: {e}"));
            }
        }
    }
}

fn benchmark_with_github_source(stdout: &str, source: &GithubSource) -> Result<String, String> {
    let mut value: serde_json::Value =
        serde_json::from_str(stdout).map_err(|e| format!("invalid JSON: {e}"))?;
    let Some(obj) = value.as_object_mut() else {
        return Err("benchmark output root is not an object".to_string());
    };
    obj.insert("github_source".to_string(), github_source_json(source));
    serde_json::to_string_pretty(&value).map_err(|e| e.to_string())
}

fn json_with_github_source(stdout: &str, source: &GithubSource) -> Result<String, String> {
    let mut value: serde_json::Value =
        serde_json::from_str(stdout).map_err(|e| format!("invalid JSON: {e}"))?;
    let Some(obj) = value.as_object_mut() else {
        return Err("gate output root is not an object".to_string());
    };
    obj.insert("github_source".to_string(), github_source_json(source));
    serde_json::to_string_pretty(&value).map_err(|e| e.to_string())
}

fn github_source_json(source: &GithubSource) -> serde_json::Value {
    serde_json::json!({
        "repo": source.repo,
        "url": source.url,
        "requested_ref": source.requested_ref,
        "resolved_commit": source.resolved_commit,
        "file_count": source.file_count,
        "byte_size": source.byte_size,
        "has_submodules": source.has_submodules,
        "has_lfs": source.has_lfs,
        "checkout_path": source.checkout_path.display().to_string(),
        "cleanup": source.cleanup,
    })
}

fn github_source_block(source: &GithubSource) -> String {
    format!(
        "\nSOURCE:\n- github_repo: {}\n- github_url: {}\n- requested_ref: {}\n- resolved_commit: {}\n- file_count: {}\n- byte_size: {}\n- has_submodules: {}\n- has_lfs: {}\n- checkout_path: {}\n- cleanup: {}\n",
        source.repo,
        source.url,
        source.requested_ref,
        source.resolved_commit,
        source.file_count,
        source.byte_size,
        source.has_submodules,
        source.has_lfs,
        source.checkout_path.display(),
        source.cleanup
    )
}

fn cleanup_checkout(temp_root: &PathBuf, keep: bool) {
    if keep {
        eprintln!("github checkout preserved: {}", temp_root.display());
        return;
    }
    match std::fs::remove_dir_all(temp_root) {
        Ok(()) => eprintln!("github checkout removed: {}", temp_root.display()),
        Err(e) => eprintln!(
            "github checkout cleanup failed: {}: {e}",
            temp_root.display()
        ),
    }
}

fn exit_code_from_output(output: &Output) -> ExitCode {
    match output.status.code() {
        Some(code) => ExitCode::from(u8::try_from(code).unwrap_or(1)),
        None => ExitCode::FAILURE,
    }
}

#[derive(Serialize)]
struct BenchmarkReport {
    schema_version: u8,
    repo: BenchmarkRepo,
    scan: BenchmarkScan,
    memory: BenchmarkMemory,
    seed: BenchmarkSeed,
    model: BenchmarkModel,
    tokens: BenchmarkTokens,
    cost: BenchmarkCost,
    notes: Vec<&'static str>,
}

#[derive(Serialize)]
struct BenchmarkRepo {
    path: String,
    name: String,
}

#[derive(Serialize)]
struct BenchmarkScan {
    candidate_files: usize,
    dehydrated_files: usize,
    unsupported_files: usize,
    read_failed: usize,
    parse_failed: usize,
    serialization_failed: usize,
    wall_clock_ms: u128,
    total_wall_clock_ms: u128,
    max_file_bytes: u64,
    concurrency: usize,
}

#[derive(Serialize)]
struct BenchmarkMemory {
    resident_set_peak_kib: Option<u64>,
    source: String,
}

#[derive(Serialize)]
struct BenchmarkSeed {
    records_sent: usize,
    candidate_bytes: usize,
    bytes_sent: usize,
    seed_cap_bytes: usize,
    reduce_batches: usize,
    record_truncated: usize,
    truncated_records: Vec<report::TruncatedRecord>,
    suspicious_artifacts: Vec<report::SuspiciousArtifact>,
}

#[derive(Serialize)]
struct BenchmarkModel {
    small_model: BenchmarkSmallModel,
    large_model: BenchmarkLargeModel,
}

#[derive(Serialize)]
struct BenchmarkSmallModel {
    chunks_total: usize,
    attempts_total: usize,
    chunks_failed: usize,
    skipped_no_model: bool,
}

#[derive(Serialize)]
struct BenchmarkLargeModel {
    calls: usize,
    reduce_batches_planned: usize,
}

#[derive(Serialize)]
struct BenchmarkTokens {
    estimation: &'static str,
    estimated_input_tokens: u64,
    estimated_output_tokens: u64,
}

#[derive(Serialize)]
struct BenchmarkCost {
    currency: &'static str,
    configured: bool,
    input_per_million: Option<f64>,
    output_per_million: Option<f64>,
    estimated_input_cost: Option<f64>,
    estimated_output_cost: Option<f64>,
    estimated_total_cost: Option<f64>,
}

impl BenchmarkReport {
    fn from_run(
        cfg: &Config,
        coverage: &InputCoverage,
        scan_elapsed_ms: u128,
        small_model_chunks_total: usize,
        total_elapsed_ms: u128,
        memory: BenchmarkMemory,
    ) -> Self {
        let estimated_input_tokens = estimate_tokens_from_bytes(coverage.seed_bytes);
        let estimated_output_tokens = cfg.benchmark_estimated_output_tokens;
        let cost = BenchmarkCost::new(
            estimated_input_tokens,
            estimated_output_tokens,
            cfg.benchmark_input_1m_cost,
            cfg.benchmark_output_1m_cost,
        );
        Self {
            schema_version: 1,
            repo: BenchmarkRepo {
                path: cfg.root.display().to_string(),
                name: cfg
                    .root
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("")
                    .to_string(),
            },
            scan: BenchmarkScan {
                candidate_files: coverage.scan.candidate_files,
                dehydrated_files: coverage.dehydrated,
                unsupported_files: coverage.scan.unsupported_files,
                read_failed: coverage.scan.read_failed,
                parse_failed: coverage.scan.parse_failed,
                serialization_failed: coverage.scan.serialization_failed,
                wall_clock_ms: scan_elapsed_ms,
                total_wall_clock_ms: total_elapsed_ms,
                max_file_bytes: cfg.max_bytes,
                concurrency: cfg.concurrency,
            },
            memory,
            seed: BenchmarkSeed {
                records_sent: coverage.seeded,
                candidate_bytes: coverage.candidate_seed_bytes,
                bytes_sent: coverage.seed_bytes,
                seed_cap_bytes: coverage.seed_cap,
                reduce_batches: coverage.batches,
                record_truncated: coverage.record_truncated,
                truncated_records: coverage
                    .truncated_records
                    .iter()
                    .take(20)
                    .cloned()
                    .collect(),
                suspicious_artifacts: coverage
                    .suspicious_artifacts
                    .iter()
                    .take(20)
                    .cloned()
                    .collect(),
            },
            model: BenchmarkModel {
                small_model: BenchmarkSmallModel {
                    chunks_total: small_model_chunks_total,
                    attempts_total: 0,
                    chunks_failed: 0,
                    skipped_no_model: true,
                },
                large_model: BenchmarkLargeModel {
                    calls: 0,
                    reduce_batches_planned: coverage.batches,
                },
            },
            tokens: BenchmarkTokens {
                estimation: "ceil(seed_bytes_sent / 4); tokenizer-free approximation",
                estimated_input_tokens,
                estimated_output_tokens,
            },
            cost,
            notes: vec![
                "benchmark mode performs no model calls",
                "token and cost values are estimates unless provider usage data is supplied externally",
            ],
        }
    }
}

impl BenchmarkCost {
    fn new(
        input_tokens: u64,
        output_tokens: u64,
        input_per_million: Option<f64>,
        output_per_million: Option<f64>,
    ) -> Self {
        let input = estimate_cost(input_tokens, input_per_million);
        let output = estimate_cost(output_tokens, output_per_million);
        let configured = input_per_million.is_some() || output_per_million.is_some();
        Self {
            currency: "USD",
            configured,
            input_per_million,
            output_per_million,
            estimated_input_cost: input,
            estimated_output_cost: output,
            estimated_total_cost: if configured {
                Some(input.unwrap_or(0.0) + output.unwrap_or(0.0))
            } else {
                None
            },
        }
    }
}

fn estimate_tokens_from_bytes(bytes: usize) -> u64 {
    u64::try_from(bytes.saturating_add(3) / 4).unwrap_or(u64::MAX)
}

fn estimate_cost(tokens: u64, per_million: Option<f64>) -> Option<f64> {
    per_million.map(|price| (tokens as f64 / 1_000_000.0) * price)
}

fn elapsed_ms(started: Instant) -> u128 {
    started.elapsed().as_millis()
}

fn resident_memory_metric() -> BenchmarkMemory {
    #[cfg(target_os = "linux")]
    {
        if let Some(value) = linux_status_kib("VmHWM") {
            return BenchmarkMemory {
                resident_set_peak_kib: Some(value),
                source: "procfs:VmHWM".to_string(),
            };
        }
        if let Some(value) = linux_status_kib("VmRSS") {
            return BenchmarkMemory {
                resident_set_peak_kib: Some(value),
                source: "procfs:VmRSS".to_string(),
            };
        }
    }
    BenchmarkMemory {
        resident_set_peak_kib: None,
        source: "unavailable".to_string(),
    }
}

#[cfg(target_os = "linux")]
fn linux_status_kib(key: &str) -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        let Some(value) = line
            .strip_prefix(key)
            .and_then(|rest| rest.strip_prefix(':'))
        else {
            continue;
        };
        return value
            .split_whitespace()
            .next()
            .and_then(|n| n.parse::<u64>().ok());
    }
    None
}

#[derive(Default)]
struct ScanStats {
    candidate_files: usize,
    read_failed: usize,
    unsupported_files: usize,
    parse_failed: usize,
    serialization_failed: usize,
}

struct InputCoverage {
    scan: ScanStats,
    dehydrated: usize,
    seeded: usize,
    seed_bytes: usize,
    candidate_seed_bytes: usize,
    seed_cap: usize,
    record_truncated: usize,
    truncated_records: Vec<report::TruncatedRecord>,
    suspicious_artifacts: Vec<report::SuspiciousArtifact>,
    batches: usize,
}

impl InputCoverage {
    fn model_context(&self) -> String {
        format!(
            "INPUT_COVERAGE:\n- candidate_files: {}\n- dehydrated_files: {}\n- records_sent_to_models: {}\n- seed_bytes_sent: {}\n- candidate_seed_bytes: {}\n- seed_cap_bytes: {}\n- reduce_batches: {}\n- record_truncated: {}",
            self.scan.candidate_files,
            self.dehydrated,
            self.seeded,
            self.seed_bytes,
            self.candidate_seed_bytes,
            self.seed_cap,
            self.batches,
            self.record_truncated
        )
    }

    fn markdown_section(&self, language: ReportLanguage) -> String {
        let status = if self.record_truncated > 0 {
            "COMPLETE_WITH_RECORD_COMPRESSION"
        } else {
            "COMPLETE"
        };
        let scope_note = if self.record_truncated > 0 {
            match language {
                ReportLanguage::En => {
                    "\n\nResult scope: all compact records were batched; some oversized records were compressed further."
                }
                ReportLanguage::Zh => {
                    "\n\n\u{7ed3}\u{679c}\u{8303}\u{56f4}\u{ff1a}\u{6240}\u{6709}\u{7d27}\u{51d1}\u{8bb0}\u{5f55}\u{5747}\u{5df2}\u{5206}\u{6279}\u{ff1b}\u{90e8}\u{5206}\u{8d85}\u{5927}\u{8bb0}\u{5f55}\u{8fdb}\u{4e00}\u{6b65}\u{538b}\u{7f29}\u{3002}"
                }
            }
        } else {
            ""
        };
        let heading = match language {
            ReportLanguage::En => "Input Coverage",
            ReportLanguage::Zh => "\u{8f93}\u{5165}\u{8986}\u{76d6}",
        };
        let table_header = match language {
            ReportLanguage::En => {
                "| Status | Candidate Files | Dehydrated Files | Model Records | Reduce Batches | Seed Bytes | Candidate Seed Bytes | Cap Bytes | Record Truncated |\n"
            }
            ReportLanguage::Zh => {
                "|\u{72b6}\u{6001}|\u{5019}\u{9009}\u{6587}\u{4ef6}|\u{8131}\u{6c34}\u{6587}\u{4ef6}|\u{6a21}\u{578b}\u{8bb0}\u{5f55}|Reduce \u{6279}\u{6b21}|Seed \u{5b57}\u{8282}|\u{5019}\u{9009} Seed \u{5b57}\u{8282}|\u{4e0a}\u{9650}\u{5b57}\u{8282}|\u{8bb0}\u{5f55}\u{622a}\u{65ad}|\n"
            }
        };
        format!(
            "## {heading}\n\n{table_header}|---|---:|---:|---:|---:|---:|---:|---:|---:|\n| {status} | {} | {} | {} | {} | {} | {} | {} | {} |{scope_note}",
            self.scan.candidate_files,
            self.dehydrated,
            self.seeded,
            self.batches,
            self.seed_bytes,
            self.candidate_seed_bytes,
            self.seed_cap,
            self.record_truncated
        )
    }

    fn agent_gate_coverage(&self) -> report::AgentGateCoverage {
        report::AgentGateCoverage {
            candidate_files: self.scan.candidate_files,
            dehydrated_files: self.dehydrated,
            read_failed: self.scan.read_failed,
            unsupported_files: self.scan.unsupported_files,
            parse_failed: self.scan.parse_failed,
            serialization_failed: self.scan.serialization_failed,
            record_truncated: self.record_truncated,
            seed_bytes: self.seed_bytes,
            truncated_records: self.truncated_records.clone(),
            suspicious_artifacts: self.suspicious_artifacts.clone(),
        }
    }
}

fn diagnostics_section(
    coverage: &InputCoverage,
    map: Option<&model::MapReport>,
    language: ReportLanguage,
) -> String {
    let mut s = match language {
        ReportLanguage::En => String::from("## Diagnostics\n\n"),
        ReportLanguage::Zh => String::from("## \u{8bca}\u{65ad}\n\n"),
    };
    s.push_str("| Area | Metric | Value |\n");
    s.push_str("|---|---|---:|\n");
    s.push_str(&format!(
        "| scan | read_failed | {} |\n",
        coverage.scan.read_failed
    ));
    s.push_str(&format!(
        "| scan | unsupported_files | {} |\n",
        coverage.scan.unsupported_files
    ));
    s.push_str(&format!(
        "| scan | parse_failed | {} |\n",
        coverage.scan.parse_failed
    ));
    s.push_str(&format!(
        "| scan | serialization_failed | {} |\n",
        coverage.scan.serialization_failed
    ));
    if let Some(report) = map {
        s.push_str(&format!(
            "| small_model_map | chunks_total | {} |\n",
            report.chunks_total
        ));
        s.push_str(&format!(
            "| small_model_map | chunks_succeeded | {} |\n",
            report.chunks_succeeded
        ));
        s.push_str(&format!(
            "| small_model_map | chunks_failed | {} |\n",
            report.chunks_failed
        ));
        s.push_str(&format!(
            "| small_model_map | attempts_total | {} |\n",
            report.attempts_total
        ));
        s.push_str(&format!(
            "| small_model_map | retry_attempts | {} |\n",
            report.retry_attempts
        ));
        s.push_str(&format!(
            "| small_model_map | skipped_no_model | {} |\n",
            report.skipped_no_model
        ));
    }
    s.push_str(&format!("| reduce | batches | {} |\n", coverage.batches));
    s.push_str(&format!(
        "| reduce | record_truncated | {} |\n",
        coverage.record_truncated
    ));
    s
}

fn escape_markdown_cell(s: &str) -> String {
    s.replace('`', "\\`").replace('\n', " ")
}

#[derive(Serialize)]
struct CompactSeedRecord {
    path: String,
    lang: Option<&'static str>,
    signatures: Vec<String>,
    calls: Vec<String>,
    locations: Vec<CompactSeedLocation>,
    external: Vec<String>,
    omitted: CompactOmitted,
}

#[derive(Serialize)]
struct CompactSeedLocation {
    kind: &'static str,
    line: usize,
    text: String,
}

/// Display a scanned file relative to the audit root. Falls back to the original
/// path when the prefix does not match (e.g. unusual symlink layouts); this
/// never panics because `strip_prefix` errors degrade to the absolute path.
fn audit_relative_path<'a>(path: &'a Path, root: &Path) -> &'a Path {
    path.strip_prefix(root).unwrap_or(path)
}

fn inspect_suspicious_artifact(
    path: &str,
    size_bytes: u64,
    meta: &std::fs::Metadata,
    skipped_for_size: bool,
) -> Option<report::SuspiciousArtifact> {
    let lower = path.replace('\\', "/").to_ascii_lowercase();
    let file_name = lower.rsplit('/').next().unwrap_or(lower.as_str());
    let mut reasons = Vec::new();
    if skipped_for_size {
        reasons.push("large_opaque_file");
    }
    if is_binary_or_archive_name(file_name) {
        reasons.push("binary_or_archive_extension");
    }
    if is_release_or_install_path(&lower) && is_executable(meta) {
        reasons.push("executable_in_install_or_release_path");
    } else if is_executable(meta) && !looks_like_text_script(file_name) {
        reasons.push("extensionless_or_binary_executable");
    }
    if reasons.is_empty() {
        return None;
    }
    Some(report::SuspiciousArtifact {
        path: path.to_string(),
        size_bytes,
        reason: reasons.join(","),
    })
}

fn is_binary_or_archive_name(file_name: &str) -> bool {
    [
        ".dylib", ".so", ".dll", ".exe", ".bin", ".wasm", ".jar", ".class", ".a", ".o", ".tar",
        ".tgz", ".gz", ".zip", ".xz", ".7z", ".rar", ".pkg", ".dmg",
    ]
    .iter()
    .any(|suffix| file_name.ends_with(suffix))
}

fn is_release_or_install_path(path: &str) -> bool {
    path.contains("release")
        || path.contains("dist/")
        || path.contains("bin/")
        || path.contains("install")
        || path.contains("scripts/")
}

fn looks_like_text_script(file_name: &str) -> bool {
    [
        ".sh", ".bash", ".zsh", ".py", ".rb", ".pl", ".js", ".ts", ".lua",
    ]
    .iter()
    .any(|suffix| file_name.ends_with(suffix))
}

#[cfg(unix)]
fn is_executable(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_: &std::fs::Metadata) -> bool {
    false
}

#[derive(Serialize, Default)]
struct CompactOmitted {
    signatures: usize,
    calls: usize,
    locations: usize,
    external: usize,
}

struct SeedRecord {
    path: String,
    json: String,
    truncated: bool,
    original_bytes: usize,
    reason: String,
}

fn compact_seed_record(sum: &extract::AstSummary, cap: usize) -> Option<SeedRecord> {
    let mut sig_limit = 24usize;
    let mut call_limit = 80usize;
    let mut loc_limit = 120usize;
    let mut ext_limit = 24usize;
    let mut text_limit = 120usize;

    loop {
        let record = compact_seed_record_with_limits(
            sum, sig_limit, call_limit, loc_limit, ext_limit, text_limit,
        );
        let json = serde_json::to_string(&record).ok()?;
        if json.len().saturating_add(1) <= cap {
            return Some(SeedRecord {
                path: sum.path.clone(),
                json,
                truncated: record.omitted.signatures
                    + record.omitted.calls
                    + record.omitted.locations
                    + record.omitted.external
                    > 0,
                original_bytes: serde_json::to_string(sum)
                    .map(|json| json.len())
                    .unwrap_or(0),
                reason: "compact_record_limits".to_string(),
            });
        }
        if loc_limit > 20 {
            loc_limit /= 2;
        } else if call_limit > 20 {
            call_limit /= 2;
        } else if sig_limit > 8 {
            sig_limit /= 2;
        } else if ext_limit > 8 {
            ext_limit /= 2;
        } else if text_limit > 60 {
            text_limit /= 2;
        } else {
            let minimal = compact_seed_record_with_limits(sum, 4, 8, 8, 4, 48);
            return serde_json::to_string(&minimal).ok().map(|json| SeedRecord {
                path: sum.path.clone(),
                json,
                truncated: true,
                original_bytes: serde_json::to_string(sum)
                    .map(|json| json.len())
                    .unwrap_or(0),
                reason: "minimal_record_after_size_cap".to_string(),
            });
        }
    }
}

fn compact_seed_record_with_limits(
    sum: &extract::AstSummary,
    sig_limit: usize,
    call_limit: usize,
    loc_limit: usize,
    ext_limit: usize,
    text_limit: usize,
) -> CompactSeedRecord {
    CompactSeedRecord {
        path: sum.path.clone(),
        lang: sum.lang,
        signatures: take_strings(&sum.signatures, sig_limit, text_limit),
        calls: take_strings(&sum.calls, call_limit, text_limit),
        locations: sum
            .locations
            .iter()
            .take(loc_limit)
            .map(|loc| CompactSeedLocation {
                kind: loc.kind,
                line: loc.line,
                text: truncate_text(&loc.text, text_limit),
            })
            .collect(),
        external: take_strings(&sum.external, ext_limit, text_limit),
        omitted: CompactOmitted {
            signatures: sum.signatures.len().saturating_sub(sig_limit),
            calls: sum.calls.len().saturating_sub(call_limit),
            locations: sum.locations.len().saturating_sub(loc_limit),
            external: sum.external.len().saturating_sub(ext_limit),
        },
    }
}

fn take_strings(values: &[String], limit: usize, text_limit: usize) -> Vec<String> {
    values
        .iter()
        .take(limit)
        .map(|s| truncate_text(s, text_limit))
        .collect()
}

fn truncate_text(value: &str, limit: usize) -> String {
    let mut out: String = value.chars().take(limit).collect();
    if value.chars().count() > limit {
        out.push_str("...");
    }
    out
}

fn seed_batches(records: &[String], cap: usize) -> Vec<String> {
    let mut batches = Vec::new();
    let mut current = String::new();
    for record in records {
        let record_bytes = record.len().saturating_add(1);
        if !current.is_empty() && current.len().saturating_add(record_bytes) > cap {
            batches.push(current);
            current = String::new();
        }
        current.push_str(record);
        current.push('\n');
    }
    if !current.is_empty() {
        batches.push(current);
    }
    batches
}

fn build_react_batches(
    coverage: &InputCoverage,
    seed_batches: &[String],
    language: ReportLanguage,
) -> Vec<String> {
    if seed_batches.is_empty() {
        return vec![format!(
            "{}\n- report_language: {}\n- report_language_instruction: {}\n\nAST_SEED:\n",
            coverage.model_context(),
            language.code(),
            language.prompt_instruction()
        )];
    }
    seed_batches
        .iter()
        .enumerate()
        .map(|(idx, batch)| {
            let batch_context = format!(
                "{}\n- report_language: {}\n- report_language_instruction: {}\n- current_reduce_batch: {}\n- reduce_batches_total: {}\n- current_batch_seed_bytes: {}",
                coverage.model_context(),
                language.code(),
                language.prompt_instruction(),
                idx + 1,
                seed_batches.len(),
                batch.len()
            );
            format!("{batch_context}\n\nAST_SEED_BATCH:\n{batch}")
        })
        .collect()
}

struct BatchReport {
    idx: usize,
    bytes: usize,
    markdown: String,
}

fn render_batch_reports(reports: &[BatchReport], language: ReportLanguage) -> String {
    if reports.len() == 1 {
        return reports
            .first()
            .map(|report| report.markdown.clone())
            .unwrap_or_default();
    }
    let mut s = match language {
        ReportLanguage::En => String::from("## Converged Reduce Results\n\n"),
        ReportLanguage::Zh => String::from("## \u{6c47}\u{603b} Reduce \u{7ed3}\u{679c}\n\n"),
    };
    let mut rows = BTreeSet::new();
    for report in reports {
        for line in report.markdown.lines() {
            let trimmed = line.trim();
            if !trimmed.starts_with('|')
                || trimmed.contains("---")
                || trimmed.contains("Severity")
                || trimmed.contains("\u{4e25}\u{91cd}\u{6027}")
            {
                continue;
            }
            rows.insert(trimmed.to_string());
        }
    }
    if rows.is_empty() {
        s.push_str(match language {
            ReportLanguage::En => {
                "No model findings survived cross-batch convergence. Review the authoritative deterministic ledger above.\n"
            }
            ReportLanguage::Zh => {
                "\u{8de8}\u{6279}\u{6b21}\u{6c47}\u{603b}\u{540e}\u{65e0}\u{6a21}\u{578b}\u{98ce}\u{9669}\u{9879}\u{3002}\u{8bf7}\u{4ee5}\u{4e0a}\u{65b9}\u{786e}\u{5b9a}\u{6027}\u{53f0}\u{8d26}\u{4e3a}\u{51c6}\u{3002}\n"
            }
        });
        return s;
    }
    s.push_str(match language {
        ReportLanguage::En => "| Severity | Scope | Location | Rule | Finding |\n",
        ReportLanguage::Zh => {
            "|\u{4e25}\u{91cd}\u{6027}|\u{4f5c}\u{7528}\u{57df}|\u{4f4d}\u{7f6e}|\u{89c4}\u{5219}|\u{53d1}\u{73b0}|\n"
        }
    });
    s.push_str("|---|---|---|---|---|\n");
    for row in rows {
        s.push_str(&row);
        s.push('\n');
    }
    s
}

fn render_partial_reports(
    finals: &[BatchReport],
    partials: &[BatchReport],
    language: ReportLanguage,
) -> String {
    let mut s = match language {
        ReportLanguage::En => String::from("## Reduce Batch Status\n\n"),
        ReportLanguage::Zh => String::from("## Reduce \u{6279}\u{6b21}\u{72b6}\u{6001}\n\n"),
    };
    if !finals.is_empty() {
        s.push_str(match language {
            ReportLanguage::En => "### Completed Batches\n\n",
            ReportLanguage::Zh => "### \u{5df2}\u{5b8c}\u{6210}\u{6279}\u{6b21}\n\n",
        });
        s.push_str(&render_batch_reports(finals, language));
        s.push('\n');
    }
    if !partials.is_empty() {
        s.push_str(match language {
            ReportLanguage::En => "### Failed Or Partial Batches\n\n",
            ReportLanguage::Zh => {
                "### \u{5931}\u{8d25}\u{6216}\u{90e8}\u{5206}\u{5b8c}\u{6210}\u{6279}\u{6b21}\n\n"
            }
        });
        let batch_label = match language {
            ReportLanguage::En => "Batch",
            ReportLanguage::Zh => "\u{6279}\u{6b21}",
        };
        for report in partials {
            s.push_str(&format!(
                "- {batch_label} {} ({} bytes): `{}`\n",
                report.idx + 1,
                report.bytes,
                escape_markdown_cell(&report.markdown)
            ));
        }
    }
    s
}

fn should_log_scan_progress(candidate_files: usize, debug: bool) -> bool {
    let interval = if debug { 25 } else { 100 };
    candidate_files > 0 && candidate_files.is_multiple_of(interval)
}

fn log_scan_progress(scan: &ScanStats, dehydrated: usize, started: Instant, debug: bool) {
    if should_log_scan_progress(scan.candidate_files, debug) {
        eprintln!(
            "scan progress: candidate_files: {}  dehydrated_files: {}  unsupported_files: {}  parse_failed: {}  elapsed_ms: {}",
            scan.candidate_files,
            dehydrated,
            scan.unsupported_files,
            scan.parse_failed,
            started.elapsed().as_millis()
        );
    }
}

fn audit_result_heading(language: ReportLanguage) -> &'static str {
    match language {
        ReportLanguage::En => "Audit Result",
        ReportLanguage::Zh => "\u{5ba1}\u{8ba1}\u{7ed3}\u{679c}",
    }
}

/// Heading for the authoritative, model-independent deterministic ledger.
fn deterministic_ledger_heading(language: ReportLanguage) -> &'static str {
    match language {
        ReportLanguage::En => "Deterministic Risk Ledger (authoritative)",
        ReportLanguage::Zh => {
            "\u{786e}\u{5b9a}\u{6027}\u{98ce}\u{9669}\u{53f0}\u{8d26}\u{ff08}\u{6743}\u{5a01}\u{6765}\u{6e90}\u{ff09}"
        }
    }
}

/// Note clarifying that the deterministic ledger is the source of truth and the
/// model section below is interpretation, not a competing set of findings.
fn deterministic_ledger_note(language: ReportLanguage) -> &'static str {
    match language {
        ReportLanguage::En => {
            "These findings come from sift's deterministic rules and are the source of truth. Severity is capped by path scope (test and fixture paths cannot exceed Low). The model interpretation below is supplementary."
        }
        ReportLanguage::Zh => {
            "\u{4ee5}\u{4e0b}\u{53d1}\u{73b0}\u{7531} sift \u{786e}\u{5b9a}\u{6027}\u{89c4}\u{5219}\u{751f}\u{6210}\u{ff0c}\u{4e3a}\u{6743}\u{5a01}\u{6765}\u{6e90}\u{3002}\u{4e25}\u{91cd}\u{6027}\u{6309}\u{8def}\u{5f84}\u{4f5c}\u{7528}\u{57df}\u{5c01}\u{9876}\u{ff08}\u{6d4b}\u{8bd5}\u{4e0e}\u{5939}\u{5177}\u{8def}\u{5f84}\u{4e0d}\u{8d85}\u{8fc7} Low\u{ff09}\u{3002}\u{4e0b}\u{65b9}\u{6a21}\u{578b}\u{89e3}\u{8bfb}\u{4ec5}\u{4f5c}\u{8865}\u{5145}\u{3002}"
        }
    }
}

/// Heading for the supplementary model narrative.
fn model_interpretation_heading(language: ReportLanguage) -> &'static str {
    match language {
        ReportLanguage::En => "Model Interpretation",
        ReportLanguage::Zh => "\u{6a21}\u{578b}\u{89e3}\u{8bfb}",
    }
}

fn incomplete_audit_heading(language: ReportLanguage) -> &'static str {
    match language {
        ReportLanguage::En => "Incomplete Audit",
        ReportLanguage::Zh => "\u{672a}\u{5b8c}\u{6210}\u{5ba1}\u{8ba1}",
    }
}

fn incomplete_audit_notice(language: ReportLanguage) -> &'static str {
    match language {
        ReportLanguage::En => {
            "One or more large-model Reduce batches failed or hit a bound. This output is not a completed audit verdict."
        }
        ReportLanguage::Zh => {
            "\u{4e00}\u{4e2a}\u{6216}\u{591a}\u{4e2a}\u{5927}\u{6a21}\u{578b} Reduce \u{6279}\u{6b21}\u{5931}\u{8d25}\u{6216}\u{89e6}\u{8fbe}\u{8fb9}\u{754c}\u{3002}\u{6b64}\u{8f93}\u{51fa}\u{4e0d}\u{662f}\u{5b8c}\u{6574}\u{5ba1}\u{8ba1}\u{7ed3}\u{8bba}\u{3002}"
        }
    }
}

fn local_fallback_heading(language: ReportLanguage) -> &'static str {
    match language {
        ReportLanguage::En => "Local Deterministic Fallback",
        ReportLanguage::Zh => "\u{672c}\u{5730}\u{786e}\u{5b9a}\u{6027}\u{56de}\u{9000}",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_key_hint_stays_available_to_main() {
        assert!(config::missing_large_key_hint().contains("SIFT_API_KEY"));
    }

    #[test]
    fn audit_relative_path_strips_root() {
        let root = Path::new("/abs/project");
        let file = Path::new("/abs/project/tests/fixtures/sample/install.sh");
        assert_eq!(
            audit_relative_path(file, root),
            Path::new("tests/fixtures/sample/install.sh")
        );
    }

    #[test]
    fn audit_relative_path_falls_back_when_outside_root() {
        let root = Path::new("/abs/project");
        let file = Path::new("/elsewhere/package.json");
        assert_eq!(audit_relative_path(file, root), file);
    }

    #[test]
    fn seed_batching_preserves_all_records() {
        let records = vec![
            r#"{"path":"a.rs","calls":[],"locations":[],"external":[]}"#.to_string(),
            r#"{"path":"b.rs","calls":[],"locations":[],"external":[]}"#.to_string(),
            r#"{"path":"c.rs","calls":[],"locations":[],"external":[]}"#.to_string(),
        ];

        let batches = seed_batches(&records, records[0].len() + 1);

        assert_eq!(batches.len(), 3);
        assert_eq!(
            batches.concat(),
            format!("{}\n{}\n{}\n", records[0], records[1], records[2])
        );
    }

    #[test]
    fn compact_seed_record_caps_oversized_files() {
        let mut summary = extract::AstSummary {
            path: "src/large.rs".to_string(),
            lang: Some("rust"),
            ..Default::default()
        };
        for idx in 0..200 {
            let text = format!("very_long_symbol_name_{idx}_{}", "x".repeat(200));
            summary.signatures.push(format!("fn {text}()"));
            summary.calls.push(format!("{text}()"));
            summary.locations.push(extract::AstLocation {
                kind: "call",
                line: idx + 1,
                text,
            });
        }

        let record = compact_seed_record(&summary, 4096);
        assert!(record.is_some(), "record serializes");
        let record = match record {
            Some(record) => record,
            None => return,
        };

        assert!(record.json.len() <= 4096);
        assert!(record.truncated);
        assert!(record.json.contains(r#""omitted""#));
    }

    #[test]
    fn react_batches_include_coverage_and_each_seed_batch() {
        let coverage = InputCoverage {
            scan: ScanStats {
                candidate_files: 2,
                ..Default::default()
            },
            dehydrated: 2,
            seeded: 2,
            seed_bytes: 12,
            candidate_seed_bytes: 12,
            seed_cap: 8,
            record_truncated: 0,
            truncated_records: Vec::new(),
            suspicious_artifacts: Vec::new(),
            batches: 2,
        };
        let seed = vec!["one\n".to_string(), "two\n".to_string()];

        let prompts = build_react_batches(&coverage, &seed, ReportLanguage::En);

        assert_eq!(prompts.len(), 2);
        assert!(prompts[0].contains("current_reduce_batch: 1"));
        assert!(prompts[0].contains("report_language: en"));
        assert!(prompts[0].contains("AST_SEED_BATCH:\none\n"));
        assert!(prompts[1].contains("current_reduce_batch: 2"));
        assert!(prompts[1].contains("AST_SEED_BATCH:\ntwo\n"));
    }

    #[test]
    fn localized_headings_render_for_zh() {
        let coverage = InputCoverage {
            scan: ScanStats {
                candidate_files: 1,
                ..Default::default()
            },
            dehydrated: 1,
            seeded: 1,
            seed_bytes: 1,
            candidate_seed_bytes: 1,
            seed_cap: 8,
            record_truncated: 0,
            truncated_records: Vec::new(),
            suspicious_artifacts: Vec::new(),
            batches: 1,
        };

        let section = coverage.markdown_section(ReportLanguage::Zh);
        assert!(section.contains("\u{8f93}\u{5165}\u{8986}\u{76d6}"));
    }

    #[test]
    fn multi_batch_reduce_renders_single_converged_table() {
        let reports = vec![
            BatchReport {
                idx: 0,
                bytes: 10,
                markdown: "# Risk Ledger\n\n| Severity | Scope | Location | Rule | Finding |\n|---|---|---|---|---|\n| HIGH | production | `src/a.rs:1` | `panic-edge` | x: `unwrap` |\n"
                    .to_string(),
            },
            BatchReport {
                idx: 1,
                bytes: 10,
                markdown: "# Risk Ledger\n\nNo deterministic risks found in the analyzed input.\n"
                    .to_string(),
            },
        ];

        let rendered = render_batch_reports(&reports, ReportLanguage::En);

        assert!(rendered.contains("Converged Reduce Results"));
        assert_eq!(
            rendered
                .matches("| Severity | Scope | Location | Rule | Finding |")
                .count(),
            1
        );
        assert!(!rendered.contains("Batch 2"));
        assert!(rendered.contains("panic-edge"));
    }

    #[test]
    fn github_repo_parser_accepts_owner_repo_and_https() {
        let parsed = parse_github_repo("jamiesun/sift");
        assert!(parsed.is_ok());
        let parsed = match parsed {
            Ok(parsed) => parsed,
            Err(_) => return,
        };
        assert_eq!(parsed.url, "https://github.com/jamiesun/sift.git");

        let parsed = parse_github_repo("https://github.com/jamiesun/sift.git");
        assert!(parsed.is_ok());
        let parsed = match parsed {
            Ok(parsed) => parsed,
            Err(_) => return,
        };
        assert_eq!(parsed.owner, "jamiesun");
        assert_eq!(parsed.repo, "sift");

        let parsed = parse_github_repo("https://github.com/jamiesun/sift.git/");
        assert!(parsed.is_ok());
        let parsed = match parsed {
            Ok(parsed) => parsed,
            Err(_) => return,
        };
        assert_eq!(parsed.url, "https://github.com/jamiesun/sift.git");
    }

    #[test]
    fn github_repo_parser_rejects_unsupported_or_authenticated_urls() {
        assert!(parse_github_repo("https://example.com/jamiesun/sift").is_err());
        assert!(parse_github_repo("https://token@github.com/jamiesun/sift").is_err());
        assert!(parse_github_repo("too/many/segments").is_err());
    }

    #[test]
    fn git_ref_arg_rejects_option_like_or_spaced_refs() {
        assert!(valid_git_ref_arg("main"));
        assert!(valid_git_ref_arg("feature/github-intake"));
        assert!(valid_git_ref_arg("0123456789abcdef"));
        assert!(!valid_git_ref_arg("--upload-pack=sh"));
        assert!(!valid_git_ref_arg("main branch"));
        assert!(!valid_git_ref_arg("main:refs/heads/side"));
        assert!(!valid_git_ref_arg("main..side"));
        assert!(!valid_git_ref_arg("main@{1}"));
    }

    #[test]
    fn checkout_inspection_reports_lfs_and_limits() {
        let root = std::env::temp_dir().join(format!(
            "sift-checkout-inspect-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(root.join("src")).ok();
        std::fs::write(root.join(".gitattributes"), "*.bin filter=lfs\n").ok();
        std::fs::write(root.join(".gitmodules"), "[submodule \"x\"]\n").ok();
        std::fs::write(root.join("src/lib.rs"), "fn main() {}\n").ok();

        let inspection =
            inspect_checkout(&root, 10, 1024 * 1024).unwrap_or_else(|_| CheckoutInspection {
                file_count: 0,
                byte_size: 0,
                has_submodules: false,
                has_lfs: false,
            });
        assert!(inspection.has_lfs);
        assert!(inspection.has_submodules);
        assert!(
            inspect_checkout(&root, 1, 1024 * 1024)
                .err()
                .unwrap_or_default()
                .contains("oversized_repo")
        );
        std::fs::remove_dir_all(root).ok();
    }
}
