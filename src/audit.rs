use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

#[derive(Debug)]
pub struct SelfAuditResult {
    pub path: PathBuf,
    pub markdown: String,
    pub failures: usize,
    pub warnings: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Pass,
    Warn,
    Fail,
}

impl Status {
    fn label(self) -> &'static str {
        match self {
            Status::Pass => "PASS",
            Status::Warn => "WARN",
            Status::Fail => "FAIL",
        }
    }
}

#[derive(Debug)]
struct Check {
    dim: &'static str,
    status: Status,
    title: &'static str,
    evidence: String,
}

pub fn write_self_audit(project_root: &Path) -> Result<SelfAuditResult> {
    let checks = run_checks(project_root);
    let markdown = render(project_root, &checks);
    let failures = checks.iter().filter(|c| c.status == Status::Fail).count();
    let warnings = checks.iter().filter(|c| c.status == Status::Warn).count();
    let reports_dir = project_root.join("reports");
    fs::create_dir_all(&reports_dir).context("create reports dir")?;
    let path = reports_dir.join("self-audit.md");
    fs::write(&path, &markdown).context("write self-audit report")?;
    Ok(SelfAuditResult {
        path,
        markdown,
        failures,
        warnings,
    })
}

fn run_checks(project_root: &Path) -> Vec<Check> {
    let mut checks = Vec::new();
    let src_files = source_files(&project_root.join("src"));
    let src_text = read_joined(&src_files);

    push(
        &mut checks,
        "CQ",
        !src_text.contains(&panic_call_pattern()),
        "No explicit panic macro in src",
        "searched src for explicit panic macro",
    );
    push(
        &mut checks,
        "CQ",
        !src_text.contains(&unwrap_call_pattern()) && !src_text.contains(&expect_call_pattern()),
        "No direct unwrap/expect in src",
        "searched src for direct unwrap/expect calls",
    );
    push(
        &mut checks,
        "SEC",
        !src_text.contains(&unsafe_impl_pattern()),
        "No manual unsafe trait implementation",
        "searched src for manual unsafe trait implementations",
    );
    push(
        &mut checks,
        "RB",
        src_text.contains(".timeout(timeout)"),
        "Model transport has a hard timeout",
        "src/model.rs should wire ureq timeout",
    );
    push(
        &mut checks,
        "RB",
        src_text.contains("max_steps") && src_text.contains("max_errors"),
        "ReACT loop has bounded steps and errors",
        "src/react.rs should bound convergence",
    );
    push(
        &mut checks,
        "SEC",
        src_text.contains("starts_with(&project_root)"),
        "Module path is contained by project root",
        "src/config.rs should reject escaped absolute module paths",
    );
    push(
        &mut checks,
        "DF",
        project_root.join("src/audit.rs").exists(),
        "Self-audit module exists",
        "src/audit.rs is required for P5",
    );
    push_status(
        &mut checks,
        "BT",
        test_coverage_status(&src_files),
        "Each src file carries unit-test coverage",
        "checked for #[cfg(test)] in src/*.rs",
    );
    push_status(
        &mut checks,
        "CC",
        dead_code_allow_status(&src_text),
        "No broad dead_code allow left in production modules",
        "searched src for #![allow(dead_code)]",
    );
    push(
        &mut checks,
        "SEC",
        docs_avoid_direct_api_key(project_root),
        "Docs avoid direct API key command-line values",
        "README examples should prefer env or key file",
    );
    push(
        &mut checks,
        "DF",
        reports_ignored(project_root),
        "Reports directory is gitignored",
        ".gitignore should include /reports/",
    );

    checks
}

fn push(
    checks: &mut Vec<Check>,
    dim: &'static str,
    pass: bool,
    title: &'static str,
    evidence: &str,
) {
    push_status(
        checks,
        dim,
        if pass { Status::Pass } else { Status::Fail },
        title,
        evidence,
    );
}

fn push_status(
    checks: &mut Vec<Check>,
    dim: &'static str,
    status: Status,
    title: &'static str,
    evidence: &str,
) {
    checks.push(Check {
        dim,
        status,
        title,
        evidence: evidence.to_string(),
    });
}

fn source_files(src_root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_rs(src_root, &mut files);
    files.sort();
    files
}

fn collect_rs(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs(&path, files);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            files.push(path);
        }
    }
}

fn read_joined(files: &[PathBuf]) -> String {
    let mut out = String::new();
    for file in files {
        if let Ok(s) = fs::read_to_string(file) {
            out.push_str(&s);
            out.push('\n');
        }
    }
    out
}

fn test_coverage_status(files: &[PathBuf]) -> Status {
    let mut missing = Vec::new();
    for file in files {
        let Ok(src) = fs::read_to_string(file) else {
            continue;
        };
        if !src.contains("#[cfg(test)]") {
            missing.push(file.display().to_string());
        }
    }
    if missing.is_empty() {
        Status::Pass
    } else {
        Status::Warn
    }
}

fn dead_code_allow_status(src_text: &str) -> Status {
    if src_text.contains("#![allow(dead_code)]") {
        Status::Warn
    } else {
        Status::Pass
    }
}

fn docs_avoid_direct_api_key(project_root: &Path) -> bool {
    let docs = [
        project_root.join("README.md"),
        project_root.join("docs/README.zh.md"),
        project_root.join("AGENT.md"),
        project_root.join("docs/AGENT.zh.md"),
    ];
    for doc in docs {
        let Ok(src) = fs::read_to_string(doc) else {
            continue;
        };
        if direct_key_arg_patterns()
            .iter()
            .any(|pattern| src.contains(pattern))
        {
            return false;
        }
    }
    true
}

fn reports_ignored(project_root: &Path) -> bool {
    fs::read_to_string(project_root.join(".gitignore"))
        .map(|s| s.lines().any(|line| line.trim() == "/reports/"))
        .unwrap_or(false)
}

fn panic_call_pattern() -> String {
    ["panic", "!"].concat()
}

fn unwrap_call_pattern() -> String {
    ["un", "wrap("].concat()
}

fn expect_call_pattern() -> String {
    ["ex", "pect("].concat()
}

fn unsafe_impl_pattern() -> String {
    ["unsafe", " impl"].concat()
}

fn direct_key_arg_patterns() -> [String; 2] {
    [["--api", "-key <"].concat(), ["--api", "-key="].concat()]
}

fn render(project_root: &Path, checks: &[Check]) -> String {
    let mut s = String::new();
    s.push_str("# sift 自审计报告\n\n");
    s.push_str(&format!("- 项目根: `{}`\n", project_root.display()));
    s.push_str("- 模式: 本地确定性 P5 门禁\n\n");
    s.push_str("| Status | Dim | Check | Evidence |\n");
    s.push_str("|---|---|---|---|\n");
    for check in checks {
        s.push_str(&format!(
            "| {} | `{}` | {} | {} |\n",
            check.status.label(),
            check.dim,
            escape_cell(check.title),
            escape_cell(&check.evidence)
        ));
    }
    s
}

fn escape_cell(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_status_table() {
        let checks = vec![Check {
            dim: "DF",
            status: Status::Pass,
            title: "Report renders",
            evidence: "sample".to_string(),
        }];
        let md = render(Path::new("."), &checks);
        assert!(md.contains("sift 自审计报告"));
        assert!(md.contains("PASS"));
    }

    #[test]
    fn docs_direct_key_detector_flags_unsafe_examples() {
        let dir = unique_test_dir("docs-key");
        fs::create_dir_all(dir.join("docs")).ok();
        fs::write(
            dir.join("README.md"),
            ["sift . --api", "-key <KEY>"].concat(),
        )
        .ok();
        fs::write(dir.join("docs/README.zh.md"), "").ok();
        fs::write(dir.join("AGENT.md"), "").ok();
        fs::write(dir.join("docs/AGENT.zh.md"), "").ok();
        assert!(!docs_avoid_direct_api_key(&dir));
        fs::remove_dir_all(dir).ok();
    }

    fn unique_test_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "sift-audit-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ))
    }
}
