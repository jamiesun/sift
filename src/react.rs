use crate::model::{CallError, ModelClient};
use crate::skills::Skill;

/// Abstraction over large-model completion so tests can inject deterministic fakes.
pub trait Completer {
    fn ask(&mut self, prompt: &str) -> Result<String, CallError>;
}

impl Completer for ModelClient {
    fn ask(&mut self, prompt: &str) -> Result<String, CallError> {
        self.complete(prompt)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Final(String),
    Partial(String),
}

pub struct ReAct {
    max_steps: u32,
    max_errors: u32,
}

impl Default for ReAct {
    fn default() -> Self {
        Self {
            max_steps: 8,
            max_errors: 3,
        }
    }
}

impl ReAct {
    #[cfg(test)]
    pub fn new(max_steps: u32, max_errors: u32) -> Self {
        Self {
            max_steps: max_steps.max(1),
            max_errors: max_errors.max(1),
        }
    }

    /// Run until <FINAL>; bounded steps/errors return a partial result instead of looping.
    pub fn run(&self, m: &mut dyn Completer, seed: &str) -> Outcome {
        let mut prompt = initial_prompt(seed);
        let mut errors = 0u32;
        let mut last = String::new();
        for _ in 0..self.max_steps {
            let reply = match m.ask(&prompt) {
                Ok(r) => r,
                Err(e) => return Outcome::Partial(partial(&last, &e.label())),
            };
            if let Some(f) = extract(&reply, "FINAL") {
                return Outcome::Final(f);
            }
            match extract(&reply, "TOOL_CALL").and_then(|j| parse_call(&j)) {
                Some((skill, input)) => {
                    let tool_input = resolve_tool_input(&input, seed);
                    let obs = skill.run(tool_input);
                    last = obs.clone();
                    prompt = observation_prompt(&obs);
                }
                None => {
                    errors += 1;
                    if errors >= self.max_errors {
                        return Outcome::Partial(partial(&last, "format_errors_exhausted"));
                    }
                    prompt =
                        "Invalid format. Return <TOOL_CALL>{json}</TOOL_CALL> or <FINAL>.".into();
                }
            }
        }
        Outcome::Partial(partial(&last, "step_cap_reached"))
    }
}

fn initial_prompt(seed: &str) -> String {
    format!(
        "You are sift's audit convergence model. Return exactly one of these formats:\n\
         1. <TOOL_CALL>{{\"skill\":\"coarse_filter\",\"input\":\"$SEED\"}}</TOOL_CALL>\n\
         2. <FINAL>Markdown risk ledger</FINAL>\n\
         Available skills: coarse_filter and converge. To analyze the AST seed below, first call coarse_filter with input=\"$SEED\". After JSON findings, call converge or return FINAL.\n\
         AST seed(JSONL):\n{seed}"
    )
}

fn observation_prompt(obs: &str) -> String {
    format!(
        "OBSERVATION:\n{obs}\n\
         Next return only <TOOL_CALL>{{\"skill\":\"converge\",\"input\":\"the OBSERVATION text above or $SEED\"}}</TOOL_CALL> or <FINAL>Markdown risk ledger</FINAL>."
    )
}

fn resolve_tool_input<'a>(input: &'a str, seed: &'a str) -> &'a str {
    let trimmed = input.trim();
    if trimmed.is_empty() || trimmed == "$SEED" {
        seed
    } else {
        input
    }
}

fn partial(last: &str, reason: &str) -> String {
    format!("[TRUNCATED: {reason}] {last}")
}

/// Extract <TAG>...</TAG> content.
fn extract(s: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let a = s.find(&open)? + open.len();
    let b = s[a..].find(&close)? + a;
    Some(s[a..b].trim().to_string())
}

/// Parse {"skill":..,"input":..}; bad JSON or unknown skills count as format failures.
fn parse_call(json: &str) -> Option<(Skill, String)> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let skill = Skill::from_name(v.get("skill")?.as_str()?)?;
    let input = v.get("input").and_then(|i| i.as_str()).unwrap_or("");
    Some((skill, input.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct Fake {
        seq: RefCell<Vec<Result<String, CallError>>>,
    }
    impl Fake {
        fn new(mut v: Vec<Result<String, CallError>>) -> Self {
            v.reverse();
            Self {
                seq: RefCell::new(v),
            }
        }
    }
    impl Completer for Fake {
        fn ask(&mut self, _: &str) -> Result<String, CallError> {
            self.seq
                .borrow_mut()
                .pop()
                .unwrap_or(Err(CallError::Network))
        }
    }

    #[test]
    fn tool_call_then_final_converges() {
        let mut f = Fake::new(vec![
            Ok("<TOOL_CALL>{\"skill\":\"coarse_filter\",\"input\":\"x\"}</TOOL_CALL>".into()),
            Ok("<FINAL>risk: none</FINAL>".into()),
        ]);
        assert_eq!(
            ReAct::default().run(&mut f, "seed"),
            Outcome::Final("risk: none".into())
        );
    }

    #[test]
    fn initial_prompt_declares_tool_protocol() {
        let p = initial_prompt("seed");
        assert!(p.contains("<TOOL_CALL>"));
        assert!(p.contains("\"$SEED\""));
        assert!(p.contains("<FINAL>"));
    }

    #[test]
    fn seed_alias_feeds_tool_observation() {
        struct Probe {
            step: u8,
        }
        impl Completer for Probe {
            fn ask(&mut self, prompt: &str) -> Result<String, CallError> {
                self.step += 1;
                if self.step == 1 {
                    return Ok(
                        "<TOOL_CALL>{\"skill\":\"coarse_filter\",\"input\":\"$SEED\"}</TOOL_CALL>"
                            .into(),
                    );
                }
                assert!(prompt.contains("panic-edge"));
                Ok("<FINAL>ok</FINAL>".into())
            }
        }

        let seed =
            r#"{"path":"src/a.rs","locations":[{"kind":"call","line":2,"text":"x.unwrap"}]}"#;
        let mut p = Probe { step: 0 };
        assert_eq!(
            ReAct::new(2, 1).run(&mut p, seed),
            Outcome::Final("ok".into())
        );
    }

    #[test]
    fn unknown_skill_trips_to_partial() {
        let mut f = Fake::new(vec![Ok(
            "<TOOL_CALL>{\"skill\":\"rm\",\"input\":\"x\"}</TOOL_CALL>".into(),
        )]);
        assert!(matches!(
            ReAct::new(8, 1).run(&mut f, "s"),
            Outcome::Partial(_)
        ));
    }

    #[test]
    fn bad_json_trips_to_partial() {
        let mut f = Fake::new(vec![Ok("<TOOL_CALL>not json</TOOL_CALL>".into())]);
        assert!(matches!(
            ReAct::new(8, 1).run(&mut f, "s"),
            Outcome::Partial(_)
        ));
    }

    #[test]
    fn model_error_yields_partial() {
        let mut f = Fake::new(vec![Err(CallError::Tripped)]);
        assert!(matches!(
            ReAct::default().run(&mut f, "s"),
            Outcome::Partial(_)
        ));
    }

    #[test]
    fn step_cap_returns_partial_not_hang() {
        let loop_call = "<TOOL_CALL>{\"skill\":\"converge\",\"input\":\"x\"}</TOOL_CALL>";
        let mut f = Fake::new((0..50).map(|_| Ok(loop_call.into())).collect());
        assert!(matches!(
            ReAct::new(3, 3).run(&mut f, "s"),
            Outcome::Partial(_)
        ));
    }
}
