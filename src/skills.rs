use crate::report;

/// Compile-time local skill set callable from the ReACT loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skill {
    /// Coarse deterministic filter over dehydrated AST rows.
    CoarseFilter,
    /// Convert observations into a rendered risk ledger.
    Converge,
}

impl Skill {
    /// Route by stable skill name. Unknown names are format failures.
    pub fn from_name(name: &str) -> Option<Skill> {
        match name.trim() {
            "coarse_filter" => Some(Skill::CoarseFilter),
            "converge" => Some(Skill::Converge),
            _ => None,
        }
    }

    /// Run locally without network access.
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
        assert!(Skill::Converge.run(&coarse).contains("Risk Ledger"));
    }
}
