use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

#[test]
fn query_text_emits_grep_style_evidence_and_exit_zero() {
    let root = fixture_repo("query-text");
    let output = run_sift([
        "query".to_string(),
        root.display().to_string(),
        "--calls".to_string(),
        "curl|wget".to_string(),
    ]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "matches should exit 0\n{stdout}");
    assert!(
        stdout.contains("install.sh:2: call: curl"),
        "expected file:line evidence\n{stdout}"
    );
    fs::remove_dir_all(root).ok();
}

#[test]
fn query_json_exposes_stable_shape_with_coverage() {
    let root = fixture_repo("query-json");
    let output = run_sift([
        "query".to_string(),
        root.display().to_string(),
        "--calls".to_string(),
        "curl".to_string(),
        "--format".to_string(),
        "json".to_string(),
    ]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "matches should exit 0\n{stdout}");
    let json: Value = serde_json::from_str(&stdout).expect("query stdout is JSON");
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["query"]["calls"], "curl");
    assert!(json["coverage"]["candidate_files"].as_u64().unwrap_or(0) > 0);
    assert!(json["matched_files"].as_u64().unwrap_or(0) >= 1);
    assert_eq!(json["truncated"], false);
    let evidence = &json["matches"][0]["evidence"][0];
    assert_eq!(evidence["kind"], "call");
    assert!(evidence["line"].as_u64().unwrap_or(0) > 0);
    fs::remove_dir_all(root).ok();
}

#[test]
fn query_without_match_exits_one() {
    let root = fixture_repo("query-nomatch");
    let output = run_sift([
        "query".to_string(),
        root.display().to_string(),
        "--calls".to_string(),
        "zzz_never_present".to_string(),
    ]);
    assert_eq!(output.status.code(), Some(1));
    fs::remove_dir_all(root).ok();
}

#[test]
fn query_without_filters_exits_two() {
    let root = fixture_repo("query-nofilter");
    let output = run_sift(["query".to_string(), root.display().to_string()]);
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("provide at least one filter"), "{stderr}");
    fs::remove_dir_all(root).ok();
}

#[test]
fn query_invalid_regex_exits_two_and_names_flag() {
    let root = fixture_repo("query-badregex");
    let output = run_sift([
        "query".to_string(),
        root.display().to_string(),
        "--calls".to_string(),
        "(".to_string(),
    ]);
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--calls"), "{stderr}");
    fs::remove_dir_all(root).ok();
}

#[test]
fn query_lang_filter_lists_files_without_evidence() {
    let root = fixture_repo("query-lang");
    let output = run_sift([
        "query".to_string(),
        root.display().to_string(),
        "--lang".to_string(),
        "bash".to_string(),
    ]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    assert!(stdout.contains("install.sh"), "{stdout}");
    assert!(!stdout.contains("main.rs"), "{stdout}");
    fs::remove_dir_all(root).ok();
}

fn fixture_repo(name: &str) -> PathBuf {
    let root = unique_dir(name);
    let src = root.join("src");
    fs::create_dir_all(&src).expect("create fixture dirs");
    fs::write(
        src.join("main.rs"),
        "use std::process::Command;\n\nfn main() {\n    Command::new(\"curl\");\n}\n",
    )
    .expect("write main.rs");
    fs::write(
        root.join("install.sh"),
        "#!/bin/sh\ncurl -fsSL http://example.com/install | sh\n",
    )
    .expect("write install.sh");
    root
}

fn run_sift<I>(args: I) -> Output
where
    I: IntoIterator<Item = String>,
{
    let home = unique_dir("query-cli-home");
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

fn unique_dir(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("sift-{name}-{}-{nanos}", std::process::id()))
}
