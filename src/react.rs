// ReACT 状态机：大模型出 <TOOL_CALL>/<FINAL>，match 路由本地技能，
// 观察回灌；坏 JSON/未知技能/连错≥N → 收口 Partial，绝不死磕(铁律2)。
#![allow(dead_code)]

use crate::model::{CallError, ModelClient};
use crate::skills::Skill;

/// 抽象“问大模型”，解耦网络，单测可注入假实现。
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
    pub fn new(max_steps: u32, max_errors: u32) -> Self {
        Self {
            max_steps: max_steps.max(1),
            max_errors: max_errors.max(1),
        }
    }

    /// 跑到 <FINAL> 即收敛；步数/错误超界即返回已积累的 Partial，不无限循环。
    pub fn run(&self, m: &mut dyn Completer, seed: &str) -> Outcome {
        let mut prompt = seed.to_string();
        let mut errors = 0u32;
        let mut last = String::new();
        for _ in 0..self.max_steps {
            let reply = match m.ask(&prompt) {
                Ok(r) => r,
                Err(_) => return Outcome::Partial(partial(&last)),
            };
            if let Some(f) = extract(&reply, "FINAL") {
                return Outcome::Final(f);
            }
            match extract(&reply, "TOOL_CALL").and_then(|j| parse_call(&j)) {
                Some((skill, input)) => {
                    let obs = skill.run(&input);
                    last = obs.clone();
                    prompt = format!("OBSERVATION:\n{obs}\n下一步或 <FINAL>。");
                }
                None => {
                    errors += 1;
                    if errors >= self.max_errors {
                        return Outcome::Partial(partial(&last));
                    }
                    prompt = "格式错误，请回 <TOOL_CALL>{json}</TOOL_CALL> 或 <FINAL>。".into();
                }
            }
        }
        Outcome::Partial(partial(&last))
    }
}

fn partial(last: &str) -> String {
    format!("[TRUNCATED] {last}")
}

/// 取 <TAG>..</TAG> 内容，缺标签返回 None。
fn extract(s: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let a = s.find(&open)? + open.len();
    let b = s[a..].find(&close)? + a;
    Some(s[a..b].trim().to_string())
}

/// 解析 {"skill":..,"input":..}；坏 JSON/未知技能 → None 计一次失败。
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
                .unwrap_or(Err(CallError::Exhausted))
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
