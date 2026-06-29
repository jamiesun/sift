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

use config::{Cli, Config};

fn main() -> ExitCode {
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
        "concurrency: {}  max_file_bytes: {}  scan_only: {}  self_audit: {}",
        cfg.concurrency, cfg.max_bytes, cfg.scan_only, cfg.self_audit
    );

    if cfg.self_audit {
        match audit::write_self_audit(&cfg.project_root) {
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
    let mut seeded = 0usize;
    let mut seed = String::new();
    let mut seed_candidate_bytes = 0usize;
    let mut seed_truncated = false;
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
                    let record_bytes = j.len().saturating_add(1);
                    seed_candidate_bytes = seed_candidate_bytes.saturating_add(record_bytes);
                    if !seed_truncated && seed.len().saturating_add(record_bytes) <= SEED_CAP {
                        seed.push_str(&j);
                        seed.push('\n');
                        seeded += 1;
                    } else {
                        seed_truncated = true;
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
        // ASTs are dropped inside dehydrate; the main loop keeps only capped JSONL seed.
    }

    eprintln!(
        "scan complete, candidate_files: {}  dehydrated_files: {}  read_failed: {}  unsupported_files: {}  parse_failed: {}",
        scan.candidate_files,
        dehydrated,
        scan.read_failed,
        scan.unsupported_files,
        scan.parse_failed
    );

    let coverage = InputCoverage {
        scan,
        dehydrated,
        seeded,
        seed_bytes: seed.len(),
        candidate_seed_bytes: seed_candidate_bytes,
        seed_cap: SEED_CAP,
        truncated: seed_truncated,
    };

    let map_report = reg
        .as_mut()
        .map(|r| r.map_small_pool(&seed, cfg.concurrency));
    let react_seed = if let Some(report) = &map_report {
        eprintln!(
            "small-model Map complete, chunks_total: {}  succeeded: {}  failed: {}  attempts: {}  retries: {}  observation_bytes: {}",
            report.chunks_total,
            report.chunks_succeeded,
            report.chunks_failed,
            report.attempts_total,
            report.retry_attempts,
            report.observations.len()
        );
        if report.observations.is_empty() {
            format!("{}\n\nAST_SEED:\n{seed}", coverage.model_context())
        } else {
            format!(
                "{}\n\nSMALL_MODEL_OBSERVATIONS:\n{}\n\nAST_SEED:\n{seed}",
                coverage.model_context(),
                report.observations
            )
        }
    } else {
        format!("{}\n\nAST_SEED:\n{seed}", coverage.model_context())
    };
    let diagnostics = diagnostics_section(&coverage, map_report.as_ref());

    // Drive ReACT only when a large model is configured.
    if let Some(large) = reg.as_mut().and_then(|r| r.large.as_mut()) {
        match react::ReAct::default().run(large, &react_seed) {
            react::Outcome::Final(rep) => println!(
                "\n# Audit Result\n\n{}\n\n{}\n\n{rep}",
                coverage.markdown_section(),
                diagnostics
            ),
            react::Outcome::Partial(rep) => {
                eprintln!("partial result due to degradation or bound: {rep}");
                println!(
                    "\n# Local Degraded Audit\n\n{}\n\n{}\n\n{}",
                    coverage.markdown_section(),
                    diagnostics,
                    report::markdown_from_seed(&seed)
                );
            }
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
    truncated: bool,
}

impl InputCoverage {
    fn model_context(&self) -> String {
        format!(
            "INPUT_COVERAGE:\n- candidate_files: {}\n- dehydrated_files: {}\n- records_sent_to_models: {}\n- seed_bytes_sent: {}\n- candidate_seed_bytes: {}\n- seed_cap_bytes: {}\n- truncated: {}",
            self.scan.candidate_files,
            self.dehydrated,
            self.seeded,
            self.seed_bytes,
            self.candidate_seed_bytes,
            self.seed_cap,
            self.truncated
        )
    }

    fn markdown_section(&self) -> String {
        let status = if self.truncated {
            "PARTIAL_TRUNCATED"
        } else {
            "COMPLETE"
        };
        let scope_note = if self.truncated {
            "\n\nResult scope: analyzed subset only; candidate model seed exceeded the cap."
        } else {
            ""
        };
        format!(
            "## Input Coverage\n\n| Status | Candidate Files | Dehydrated Files | Model Records | Seed Bytes | Candidate Seed Bytes | Cap Bytes |\n|---|---:|---:|---:|---:|---:|---:|\n| {status} | {} | {} | {} | {} | {} | {} |{scope_note}",
            self.scan.candidate_files,
            self.dehydrated,
            self.seeded,
            self.seed_bytes,
            self.candidate_seed_bytes,
            self.seed_cap
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
}
