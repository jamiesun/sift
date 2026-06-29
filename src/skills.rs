// 技能=编译期 enum+match 本地函数，无动态加载（铁律4）。
// P3 提供路由与本地观察；小模型池并发粗筛(Map)在 P4 接入。
#![allow(dead_code)]

use crate::report;

/// 大模型可调度的本地技能集合。新增技能=改这里并重编译。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skill {
    /// 粗筛：对脱水骨架做要点摘取（P4 接小模型池）。
    CoarseFilter,
    /// 收敛：把观察聚合为风险结论（P4 接大模型 Reduce）。
    Converge,
}

impl Skill {
    /// 名称路由；未知技能返回 None，由调度器记一次失败。
    pub fn from_name(name: &str) -> Option<Skill> {
        match name.trim() {
            "coarse_filter" => Some(Skill::CoarseFilter),
            "converge" => Some(Skill::Converge),
            _ => None,
        }
    }

    /// 本地执行，返回回灌给状态机的观察文本。绝不联网、绝不 panic。
    pub fn run(self, input: &str) -> String {
        match self {
            Skill::CoarseFilter => report::findings_json_from_seed(input),
            Skill::Converge => report::markdown_from_findings_json(input)
                .unwrap_or_else(|| report::markdown_from_seed(input)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_names_route() {
        assert_eq!(Skill::from_name("coarse_filter"), Some(Skill::CoarseFilter));
        assert_eq!(Skill::from_name(" converge "), Some(Skill::Converge));
    }

    #[test]
    fn unknown_name_is_none() {
        assert!(Skill::from_name("rm_rf").is_none());
    }

    #[test]
    fn run_produces_observation() {
        let seed =
            r#"{"path":"src/a.rs","locations":[{"kind":"call","line":2,"text":"x.unwrap"}]}"#;
        let coarse = Skill::CoarseFilter.run(seed);
        assert!(coarse.contains("panic-edge"));
        assert!(Skill::Converge.run(&coarse).contains("风险清单"));
    }
}
