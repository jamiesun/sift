mod audit;
mod config;
mod extract;
mod model;
mod react;
mod report;
mod scanner;
mod skills;

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use clap::Parser;
use serde::Serialize;

use config::{Cli, Config, ReportLanguage};

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

    let cli = Cli::parse();

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
    const SEED_CAP: usize = 64 * 1024;
    let mut out = std::io::stdout().lock();
    for path in rx {
        scan.candidate_files += 1;
        let Ok(src) = std::fs::read(&path) else {
            scan.read_failed += 1;
            log_scan_progress(&scan, dehydrated, scan_started, cfg.debug);
            continue;
        };
        if extract::Lang::from_path(&path).is_none() {
            scan.unsupported_files += 1;
            log_scan_progress(&scan, dehydrated, scan_started, cfg.debug);
            continue;
        }
        let Some(sum) = extract::dehydrate(&path, &src) else {
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
    let small_model_chunks_total = seed_batches(&seed_records, 16 * 1024).len();
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
        let gate = report::agent_gate_from_seed(&seed, coverage.agent_gate_coverage());
        println!("{}", gate.markdown);
        return if gate.safe_to_agent_run {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }

    if reg.is_some() {
        eprintln!("small-model Map started");
    }
    let map_report = reg
        .as_mut()
        .map(|r| r.map_small_pool(&seed, cfg.concurrency));
    let observations = map_report
        .as_ref()
        .map(|report| report.observations.as_str())
        .unwrap_or("");
    let react_batches =
        build_react_batches(&coverage, observations, &seed_batches, cfg.report_language);
    if let Some(report) = &map_report {
        eprintln!(
            "small-model Map complete, chunks_total: {}  succeeded: {}  failed: {}  attempts: {}  retries: {}  skipped_no_model: {}  observation_bytes: {}",
            report.chunks_total,
            report.chunks_succeeded,
            report.chunks_failed,
            report.attempts_total,
            report.retry_attempts,
            report.skipped_no_model,
            report.observations.len()
        );
    }
    let diagnostics = diagnostics_section(&coverage, map_report.as_ref(), cfg.report_language);

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
            println!(
                "\n# {}\n\n{}\n\n{}\n\n{}",
                audit_result_heading(cfg.report_language),
                coverage.markdown_section(cfg.report_language),
                diagnostics,
                render_batch_reports(&final_reports, cfg.report_language)
            );
        } else {
            println!(
                "\n# {}\n\n{}\n\n{}\n\n{}\n\n{}\n\n## {}\n\n{}",
                incomplete_audit_heading(cfg.report_language),
                incomplete_audit_notice(cfg.report_language),
                coverage.markdown_section(cfg.report_language),
                diagnostics,
                render_partial_reports(&final_reports, &partial_reports, cfg.report_language),
                local_fallback_heading(cfg.report_language),
                report::markdown_from_seed_with_language(&seed, cfg.report_language)
            );
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
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

#[derive(Serialize, Default)]
struct CompactOmitted {
    signatures: usize,
    calls: usize,
    locations: usize,
    external: usize,
}

struct SeedRecord {
    json: String,
    truncated: bool,
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
                json,
                truncated: record.omitted.signatures
                    + record.omitted.calls
                    + record.omitted.locations
                    + record.omitted.external
                    > 0,
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
                json,
                truncated: true,
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
    observations: &str,
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
            if observations.trim().is_empty() {
                format!("{batch_context}\n\nAST_SEED_BATCH:\n{batch}")
            } else {
                format!(
                    "{batch_context}\n\nSMALL_MODEL_OBSERVATIONS_ALL_BATCHES:\n{observations}\n\nAST_SEED_BATCH:\n{batch}"
                )
            }
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
        ReportLanguage::En => String::from("## Batched Reduce Results\n\n"),
        ReportLanguage::Zh => String::from("## \u{5206}\u{6279} Reduce \u{7ed3}\u{679c}\n\n"),
    };
    for report in reports {
        let batch_label = match language {
            ReportLanguage::En => "Batch",
            ReportLanguage::Zh => "\u{6279}\u{6b21}",
        };
        s.push_str(&format!(
            "### {batch_label} {} ({} bytes)\n\n{}\n\n",
            report.idx + 1,
            report.bytes,
            report.markdown
        ));
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
            batches: 2,
        };
        let seed = vec!["one\n".to_string(), "two\n".to_string()];

        let prompts = build_react_batches(&coverage, "[]", &seed, ReportLanguage::En);

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
            batches: 1,
        };

        let section = coverage.markdown_section(ReportLanguage::Zh);
        assert!(section.contains("\u{8f93}\u{5165}\u{8986}\u{76d6}"));
    }
}
