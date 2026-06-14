use crate::config::types::ChainConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainPhase {
    Brainstorm,
    Plan,
    Code,
}

impl ChainPhase {
    pub fn from_prompt_name(name: &str) -> Option<Self> {
        match name {
            "brainstorm" => Some(ChainPhase::Brainstorm),
            "plan" => Some(ChainPhase::Plan),
            "code" => Some(ChainPhase::Code),
            _ => None,
        }
    }

    pub fn next_prompt_name(self) -> &'static str {
        match self {
            ChainPhase::Brainstorm => "plan",
            ChainPhase::Plan => "code",
            ChainPhase::Code => "review",
        }
    }

    pub fn transition_message(self) -> &'static str {
        match self {
            ChainPhase::Brainstorm => {
                "Based on the brainstorm above, create a detailed implementation plan."
            }
            ChainPhase::Plan => "Implement the plan above. Write code, tests, and verify.",
            ChainPhase::Code => {
                "Review the changes for correctness, design, testing, and security."
            }
        }
    }

    pub fn is_enabled(self, cfg: &ChainConfig) -> bool {
        match self {
            ChainPhase::Brainstorm => cfg.brainstorm_to_plan,
            ChainPhase::Plan => cfg.plan_to_code,
            ChainPhase::Code => cfg.code_to_review,
        }
    }

    pub fn chain_label(self) -> &'static str {
        match self {
            ChainPhase::Brainstorm => "Continue to plan? [Y/N/B]",
            ChainPhase::Plan => "Continue to code? [Y/N/B]",
            ChainPhase::Code => "Run /review? [Y/N/B]",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChainDecision {
    Decline,
    Accept(Option<String>),
    NotChain,
}

pub fn parse_chain_decision(input: &str) -> ChainDecision {
    let trimmed = input.trim();
    let lower = trimmed.to_lowercase();

    if lower == "n" || lower == "no" {
        return ChainDecision::Decline;
    }

    if lower == "y" || lower == "yes" {
        return ChainDecision::Accept(None);
    }

    // Match "but <msg>", "b <msg>", "yes but <msg>", etc.
    for prefix in &["but ", "b ", "yes but ", "yes b ", "y but ", "y b "] {
        if lower.starts_with(prefix) {
            let extra = trimmed[prefix.len()..].trim().to_string();
            if extra.is_empty() {
                return ChainDecision::NotChain;
            }
            return ChainDecision::Accept(Some(extra));
        }
    }

    ChainDecision::NotChain
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_phase_from_prompt_name() {
        assert_eq!(
            ChainPhase::from_prompt_name("brainstorm"),
            Some(ChainPhase::Brainstorm)
        );
        assert_eq!(ChainPhase::from_prompt_name("plan"), Some(ChainPhase::Plan));
        assert_eq!(ChainPhase::from_prompt_name("code"), Some(ChainPhase::Code));
        assert_eq!(ChainPhase::from_prompt_name("review"), None);
        assert_eq!(ChainPhase::from_prompt_name("ask"), None);
        assert_eq!(ChainPhase::from_prompt_name(""), None);
    }

    #[test]
    fn test_next_prompt_name() {
        assert_eq!(ChainPhase::Brainstorm.next_prompt_name(), "plan");
        assert_eq!(ChainPhase::Plan.next_prompt_name(), "code");
        assert_eq!(ChainPhase::Code.next_prompt_name(), "review");
    }

    #[test]
    fn test_transition_messages_are_not_empty() {
        assert!(!ChainPhase::Brainstorm.transition_message().is_empty());
        assert!(!ChainPhase::Plan.transition_message().is_empty());
        assert!(!ChainPhase::Code.transition_message().is_empty());
    }

    #[test]
    fn test_parse_decision_yes() {
        assert_eq!(parse_chain_decision("y"), ChainDecision::Accept(None));
        assert_eq!(parse_chain_decision("Y"), ChainDecision::Accept(None));
        assert_eq!(parse_chain_decision("yes"), ChainDecision::Accept(None));
        assert_eq!(parse_chain_decision("YES"), ChainDecision::Accept(None));
    }

    #[test]
    fn test_parse_decision_no() {
        assert_eq!(parse_chain_decision("n"), ChainDecision::Decline);
        assert_eq!(parse_chain_decision("no"), ChainDecision::Decline);
        assert_eq!(parse_chain_decision("N"), ChainDecision::Decline);
        assert_eq!(parse_chain_decision("NO"), ChainDecision::Decline);
    }

    #[test]
    fn test_parse_decision_but() {
        assert_eq!(
            parse_chain_decision("but add tests"),
            ChainDecision::Accept(Some("add tests".to_string()))
        );
        assert_eq!(
            parse_chain_decision("b add tests"),
            ChainDecision::Accept(Some("add tests".to_string()))
        );
        assert_eq!(
            parse_chain_decision("yes but add tests"),
            ChainDecision::Accept(Some("add tests".to_string()))
        );
        assert_eq!(
            parse_chain_decision("y but add tests"),
            ChainDecision::Accept(Some("add tests".to_string()))
        );
        assert_eq!(
            parse_chain_decision("BUT skip step 3"),
            ChainDecision::Accept(Some("skip step 3".to_string()))
        );
    }

    #[test]
    fn test_parse_decision_not_chain() {
        assert_eq!(
            parse_chain_decision("what about testing?"),
            ChainDecision::NotChain
        );
        assert_eq!(parse_chain_decision("maybe"), ChainDecision::NotChain);
        assert_eq!(parse_chain_decision(""), ChainDecision::NotChain);
    }

    #[test]
    fn test_parse_decision_but_empty_is_not_chain() {
        // "but " with only trailing whitespace — no actual instruction
        assert_eq!(parse_chain_decision("but "), ChainDecision::NotChain);
    }

    #[test]
    fn test_chain_config_defaults() {
        let cfg = ChainConfig::default();
        assert!(cfg.brainstorm_to_plan);
        assert!(cfg.plan_to_code);
        assert!(!cfg.code_to_review);
    }

    #[test]
    fn test_is_enabled_default_config() {
        let cfg = ChainConfig::default();
        assert!(ChainPhase::Brainstorm.is_enabled(&cfg));
        assert!(ChainPhase::Plan.is_enabled(&cfg));
        assert!(!ChainPhase::Code.is_enabled(&cfg));
    }

    #[test]
    fn test_is_enabled_all_off() {
        let cfg = ChainConfig {
            brainstorm_to_plan: false,
            plan_to_code: false,
            code_to_review: false,
        };
        assert!(!ChainPhase::Brainstorm.is_enabled(&cfg));
        assert!(!ChainPhase::Plan.is_enabled(&cfg));
        assert!(!ChainPhase::Code.is_enabled(&cfg));
    }

    #[test]
    fn test_is_enabled_all_on() {
        let cfg = ChainConfig {
            brainstorm_to_plan: true,
            plan_to_code: true,
            code_to_review: true,
        };
        assert!(ChainPhase::Brainstorm.is_enabled(&cfg));
        assert!(ChainPhase::Plan.is_enabled(&cfg));
        assert!(ChainPhase::Code.is_enabled(&cfg));
    }

    #[test]
    fn test_is_enabled_only_review() {
        let cfg = ChainConfig {
            brainstorm_to_plan: false,
            plan_to_code: false,
            code_to_review: true,
        };
        assert!(!ChainPhase::Brainstorm.is_enabled(&cfg));
        assert!(!ChainPhase::Plan.is_enabled(&cfg));
        assert!(ChainPhase::Code.is_enabled(&cfg));
    }

    #[test]
    fn test_full_progression_default_config() {
        // brainstorm → plan → code, code→review off by default
        let cfg = ChainConfig::default();

        let phase = ChainPhase::from_prompt_name("brainstorm").unwrap();
        assert_eq!(phase, ChainPhase::Brainstorm);
        assert!(phase.is_enabled(&cfg));
        assert_eq!(phase.next_prompt_name(), "plan");

        let phase = ChainPhase::from_prompt_name("plan").unwrap();
        assert_eq!(phase, ChainPhase::Plan);
        assert!(phase.is_enabled(&cfg));
        assert_eq!(phase.next_prompt_name(), "code");

        let phase = ChainPhase::from_prompt_name("code").unwrap();
        assert_eq!(phase, ChainPhase::Code);
        assert!(!phase.is_enabled(&cfg));
        assert_eq!(phase.next_prompt_name(), "review");
    }

    #[test]
    fn test_full_progression_all_enabled() {
        let cfg = ChainConfig {
            brainstorm_to_plan: true,
            plan_to_code: true,
            code_to_review: true,
        };

        for name in &["brainstorm", "plan", "code"] {
            let phase = ChainPhase::from_prompt_name(name).unwrap();
            assert!(phase.is_enabled(&cfg));
        }
    }

    #[test]
    fn test_parse_decision_but_variants_empty_is_not_chain() {
        // All "but" variants without actual instruction should be NotChain
        assert_eq!(parse_chain_decision("b "), ChainDecision::NotChain);
        assert_eq!(parse_chain_decision("yes but "), ChainDecision::NotChain);
        assert_eq!(parse_chain_decision("y but "), ChainDecision::NotChain);
        assert_eq!(parse_chain_decision("BUT "), ChainDecision::NotChain);
    }
}
