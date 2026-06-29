use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    High,
    Medium,
    Low,
}

impl Severity {
    fn label(self) -> &'static str {
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

pub fn findings_json_from_seed(seed: &str) -> String {
    let findings = findings_from_seed(seed);
    serde_json::to_string_pretty(&findings).unwrap_or_else(|_| "[]".to_string())
}

pub fn markdown_from_seed(seed: &str) -> String {
    render_markdown(&findings_from_seed(seed))
}

pub fn markdown_from_findings_json(input: &str) -> Option<String> {
    let findings: Vec<RiskFinding> = serde_json::from_str(input).ok()?;
    Some(render_markdown(&findings))
}

pub fn findings_from_seed(seed: &str) -> Vec<RiskFinding> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();

    for line in seed.lines() {
        let Ok(row) = serde_json::from_str::<AstRow>(line) else {
            continue;
        };

        if row.locations.is_empty() {
            for call in row.calls {
                push_call_risk(&mut out, &mut seen, &row.path, None, &call);
            }
        }

        for loc in &row.locations {
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

pub fn render_markdown(findings: &[RiskFinding]) -> String {
    let mut s = String::from("# 风险清单\n\n");
    if findings.is_empty() {
        s.push_str("未发现确定性风险。仍需人工复核动态行为、配置和跨模块语义。\n");
        return s;
    }

    s.push_str("| Severity | Location | Rule | Finding |\n");
    s.push_str("|---|---|---|---|\n");
    for f in findings {
        let location = match f.line {
            Some(line) => format!("{}:{line}", f.path),
            None => f.path.clone(),
        };
        s.push_str(&format!(
            "| {} | `{}` | `{}` | {}: `{}` |\n",
            f.severity.label(),
            escape_cell(&location),
            escape_cell(&f.rule),
            escape_cell(&f.title),
            escape_cell(&f.evidence),
        ));
    }
    s
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

fn is_panic_edge_call(call: &str) -> bool {
    let unwrap_like = call.contains(".unwrap") && !call.contains(".unwrap_or");
    unwrap_like || call.contains(".expect")
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
        let md = render_markdown(&[]);
        assert!(md.contains("未发现确定性风险"));
    }

    #[test]
    fn findings_json_round_trips_to_markdown() {
        let seed = r#"{"path":"src/a.rs","external":["[EXTERNAL_BLACKBOX] use crate::x;"]}"#;
        let json = findings_json_from_seed(seed);
        let md = markdown_from_findings_json(&json).unwrap_or_default();
        assert!(md.contains("external-blackbox"));
    }
}
