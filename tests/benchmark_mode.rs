use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

#[test]
fn benchmark_mode_outputs_stable_json_without_model_keys() {
    let output = run_sift([
        fixture("benign-controls").display().to_string(),
        "--benchmark".to_string(),
        "--benchmark-input-1m-cost".to_string(),
        "0.25".to_string(),
        "--benchmark-output-1m-cost".to_string(),
        "1.00".to_string(),
        "--benchmark-estimated-output-tokens".to_string(),
        "1000".to_string(),
    ]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "benchmark should pass\nstdout:\n{}\nstderr:\n{}",
        stdout,
        stderr
    );
    let json: Value = serde_json::from_str(&stdout).expect("benchmark stdout is JSON");
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["repo"]["name"], "benign-controls");
    assert!(json["scan"]["candidate_files"].as_u64().unwrap_or(0) > 0);
    assert!(json["scan"]["wall_clock_ms"].is_number());
    assert!(json["memory"]["source"].is_string());
    assert!(json["seed"]["bytes_sent"].as_u64().unwrap_or(0) > 0);
    assert!(json["model"]["large_model"]["calls"].as_u64().unwrap_or(1) == 0);
    assert!(
        json["tokens"]["estimated_input_tokens"]
            .as_u64()
            .unwrap_or(0)
            > 0
    );
    assert_eq!(json["tokens"]["estimated_output_tokens"], 1000);
    assert_eq!(json["cost"]["configured"], true);
    assert!(json["cost"]["estimated_total_cost"].is_number());
}

#[test]
fn benchmark_output_path_keeps_stdout_empty() {
    let dir = unique_dir("benchmark-output");
    fs::create_dir_all(&dir).expect("create temp dir");
    let out_path = dir.join("benchmark.json");
    let output = run_sift([
        fixture("benign-controls").display().to_string(),
        "--benchmark".to_string(),
        "--benchmark-output".to_string(),
        out_path.display().to_string(),
    ]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "benchmark file output should pass\nstdout:\n{}\nstderr:\n{}",
        stdout,
        stderr
    );
    assert!(stdout.trim().is_empty(), "stdout should stay empty");
    let json: Value =
        serde_json::from_str(&fs::read_to_string(&out_path).expect("read benchmark JSON"))
            .expect("benchmark output file is JSON");
    assert_eq!(json["schema_version"], 1);
    fs::remove_dir_all(dir).ok();
}

#[test]
fn scan_only_stdout_remains_jsonl_not_benchmark_json() {
    let output = run_sift([
        fixture("benign-controls").display().to_string(),
        "--scan-only".to_string(),
    ]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "scan-only should pass\nstdout:\n{}\nstderr:\n{}",
        stdout,
        stderr
    );
    assert!(!stdout.contains("\"schema_version\""));
    assert!(
        stdout
            .lines()
            .filter(|line| !line.trim().is_empty())
            .all(|line| serde_json::from_str::<Value>(line).is_ok()),
        "scan-only stdout should stay JSONL\n{}",
        stdout
    );
}

fn run_sift<I>(args: I) -> Output
where
    I: IntoIterator<Item = String>,
{
    let home = unique_dir("benchmark-home");
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
