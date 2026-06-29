mod audit;
mod config;
mod extract;
mod model;
mod react;
mod report;
mod scanner;
mod skills;

use std::io::Write;
use std::process::ExitCode;

use clap::Parser;
use serde::Serialize;

use config::{Cli, Config};

fn main() -> ExitCode {
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("doctor")) {
        return if config::run_doctor() {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }

    let cli = Cli::parse();

    if cli.self_audit {
        let project_root = match cli.target.canonicalize() {
            Ok(root) => root,
            Err(e) => {
                eprintln!(
                    "configuration error: cannot locate project root {}: {e}",
                    cli.target.display()
                );
                return ExitCode::FAILURE;
            }
        };
        eprintln!("audit root: {}", project_root.display());
        eprintln!("self_audit: true");
        match audit::write_self_audit(&project_root) {
            Ok(result) => {
                eprintln!(
                    "self-audit report: {}  FAIL: {}  WARN: {}",
                    result.path.display(),
                    result.failures,
                    result.warnings
                );
                println!("{}", result.markdown);
                return if result.failures == 0 {
                    ExitCode::SUCCESS
                } else {
                    ExitCode::FAILURE
                };
            }
            Err(e) => {
                eprintln!("self-audit failed: {e}");
                return ExitCode::FAILURE;
            }
        }
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
        "concurrency: {}  max_file_bytes: {}  scan_only: {}  self_audit: {}",
        cfg.concurrency, cfg.max_bytes, cfg.scan_only, cfg.self_audit
    );

    let mut reg = if cfg.scan_only {
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
        Some(r)
    };

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
            continue;
        };
        if extract::Lang::from_path(&path).is_none() {
            scan.unsupported_files += 1;
            continue;
        }
        let Some(sum) = extract::dehydrate(&path, &src) else {
            scan.parse_failed += 1;
            continue;
        };
        dehydrated += 1;
        match serde_json::to_string(&sum) {
            Ok(j) => {
                if reg.is_some() {
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
    }

    eprintln!(
        "scan complete, candidate_files: {}  dehydrated_files: {}  read_failed: {}  unsupported_files: {}  parse_failed: {}",
        scan.candidate_files,
        dehydrated,
        scan.read_failed,
        scan.unsupported_files,
        scan.parse_failed
    );

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
    if reg.is_some() {
        eprintln!(
            "seed prepared, records: {}  reduce_batches: {}  seed_bytes: {}  candidate_seed_bytes: {}  record_truncated: {}",
            coverage.seeded,
            coverage.batches,
            coverage.seed_bytes,
            coverage.candidate_seed_bytes,
            coverage.record_truncated
        );
    }

    let map_report = reg
        .as_mut()
        .map(|r| r.map_small_pool(&seed, cfg.concurrency));
    let observations = map_report
        .as_ref()
        .map(|report| report.observations.as_str())
        .unwrap_or("");
    let react_batches = build_react_batches(&coverage, observations, &seed_batches);
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
    let diagnostics = diagnostics_section(&coverage, map_report.as_ref());

    // Drive ReACT only when a large model is configured.
    if let Some(large) = reg.as_mut().and_then(|r| r.large.as_mut()) {
        let mut final_reports = Vec::new();
        let mut partial_reports = Vec::new();
        for (idx, react_seed) in react_batches.iter().enumerate() {
            eprintln!(
                "large-model Reduce batch {}/{} seed_bytes={}",
                idx + 1,
                react_batches.len(),
                react_seed.len()
            );
            match react::ReAct::default().run(large, react_seed) {
                react::Outcome::Final(rep) => final_reports.push(BatchReport {
                    idx,
                    bytes: react_seed.len(),
                    markdown: rep,
                }),
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
                "\n# Audit Result\n\n{}\n\n{}\n\n{}",
                coverage.markdown_section(),
                diagnostics,
                render_batch_reports(&final_reports)
            );
        } else {
            println!(
                "\n# Incomplete Audit\n\nOne or more large-model Reduce batches failed or hit a bound. This output is not a completed audit verdict.\n\n{}\n\n{}\n\n{}\n\n## Local Deterministic Fallback\n\n{}",
                coverage.markdown_section(),
                diagnostics,
                render_partial_reports(&final_reports, &partial_reports),
                report::markdown_from_seed(&seed)
            );
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
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

    fn markdown_section(&self) -> String {
        let status = if self.record_truncated > 0 {
            "COMPLETE_WITH_RECORD_COMPRESSION"
        } else {
            "COMPLETE"
        };
        let scope_note = if self.record_truncated > 0 {
            "\n\nResult scope: all compact records were batched; some oversized records were compressed further."
        } else {
            ""
        };
        format!(
            "## Input Coverage\n\n| Status | Candidate Files | Dehydrated Files | Model Records | Reduce Batches | Seed Bytes | Candidate Seed Bytes | Cap Bytes | Record Truncated |\n|---|---:|---:|---:|---:|---:|---:|---:|---:|\n| {status} | {} | {} | {} | {} | {} | {} | {} | {} |{scope_note}",
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
}

fn diagnostics_section(coverage: &InputCoverage, map: Option<&model::MapReport>) -> String {
    let mut s = String::from("## Diagnostics\n\n");
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
) -> Vec<String> {
    if seed_batches.is_empty() {
        return vec![format!("{}\n\nAST_SEED:\n", coverage.model_context())];
    }
    seed_batches
        .iter()
        .enumerate()
        .map(|(idx, batch)| {
            let batch_context = format!(
                "{}\n- current_reduce_batch: {}\n- reduce_batches_total: {}\n- current_batch_seed_bytes: {}",
                coverage.model_context(),
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

fn render_batch_reports(reports: &[BatchReport]) -> String {
    if reports.len() == 1 {
        return reports
            .first()
            .map(|report| report.markdown.clone())
            .unwrap_or_default();
    }
    let mut s = String::from("## Batched Reduce Results\n\n");
    for report in reports {
        s.push_str(&format!(
            "### Batch {} ({} bytes)\n\n{}\n\n",
            report.idx + 1,
            report.bytes,
            report.markdown
        ));
    }
    s
}

fn render_partial_reports(finals: &[BatchReport], partials: &[BatchReport]) -> String {
    let mut s = String::from("## Reduce Batch Status\n\n");
    if !finals.is_empty() {
        s.push_str("### Completed Batches\n\n");
        s.push_str(&render_batch_reports(finals));
        s.push('\n');
    }
    if !partials.is_empty() {
        s.push_str("### Failed Or Partial Batches\n\n");
        for report in partials {
            s.push_str(&format!(
                "- Batch {} ({} bytes): `{}`\n",
                report.idx + 1,
                report.bytes,
                escape_markdown_cell(&report.markdown)
            ));
        }
    }
    s
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

        let prompts = build_react_batches(&coverage, "[]", &seed);

        assert_eq!(prompts.len(), 2);
        assert!(prompts[0].contains("current_reduce_batch: 1"));
        assert!(prompts[0].contains("AST_SEED_BATCH:\none\n"));
        assert!(prompts[1].contains("current_reduce_batch: 2"));
        assert!(prompts[1].contains("AST_SEED_BATCH:\ntwo\n"));
    }
}
