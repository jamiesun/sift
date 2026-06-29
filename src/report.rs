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
        let md = render_markdown_with_language(&[], ReportLanguage::En);
        assert!(md.contains("No deterministic risks found"));
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
