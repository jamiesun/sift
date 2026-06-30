use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

#[test]
fn agent_gate_json_exposes_stable_verdict_shape() {
    let output = run_sift([
        fixture("github-action-secret-shell").display().to_string(),
        "--agent-gate".to_string(),
        "--format".to_string(),
        "json".to_string(),
    ]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success(), "gate should block\n{stdout}");
    let json: Value = serde_json::from_str(&stdout).expect("gate stdout is JSON");
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["verdict"], "CAUTION");
    assert_eq!(json["safe_to_agent_run"], false);
    assert!(json["coverage"]["candidate_files"].as_u64().unwrap_or(0) > 0);
    assert!(
        json["findings"]
            .as_array()
            .map(|findings| findings
                .iter()
                .any(|finding| finding["rule"] == "workflow-secret-shell"))
            .unwrap_or(false)
    );
}

#[test]
fn eval_corpus_reports_twenty_or_more_cases() {
    let output = run_sift(["eval-corpus".to_string()]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "eval corpus should pass\n{stdout}");
    let json: Value = serde_json::from_str(&stdout).expect("eval stdout is JSON");
    assert!(json["fixture_count"].as_u64().unwrap_or(0) >= 20);
    assert_eq!(json["failed"], 0);
}

fn run_sift<I>(args: I) -> Output
where
    I: IntoIterator<Item = String>,
{
    let home = unique_dir("agent-gate-json-home");
    fs::create_dir_all(&home).expect("create isolated HOME");
    let output = Command::new(env!("CARGO_BIN_EXE_sift"))
        .args(args)
        .env("HOME", &home)
        .env_remove("SIFT_INTERNAL_GATE")
        .env_remove("SIFT_API_KEY")
        .env_remove("SIFT_SMALL_KEY")
        .output()
        .expect("run sift");
    fs::remove_dir_all(&home).ok();
    output
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/repo-intake")
        .join(name)
}

fn unique_dir(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("sift-{name}-{}-{nanos}", std::process::id()))
}
