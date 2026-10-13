//! Built-in procedural skills embedded directly in the Corex binary.
//!
//! These skills provide foundational domain knowledge and instructions
//! for Corex features (about, fim, local-llm, mcp, sessions, slash-commands,
//! troubleshooting, yolo). They are always available even when operating
//! in empty directories or when offline without configuration.

#[derive(Debug, Clone, Copy)]
pub struct BuiltinSkill {
    pub name: &'static str,
    pub content: &'static str,
}

pub const BUILTIN_SKILLS: &[BuiltinSkill] = &[
    BuiltinSkill {
        name: "corex-about",
        content: include_str!("../skills/corex-about/SKILL.md"),
    },
    BuiltinSkill {
        name: "corex-fim",
        content: include_str!("../skills/corex-fim/SKILL.md"),
    },
    BuiltinSkill {
        name: "corex-local-llm",
        content: include_str!("../skills/corex-local-llm/SKILL.md"),
    },
    BuiltinSkill {
        name: "corex-mcp-setup",
        content: include_str!("../skills/corex-mcp-setup/SKILL.md"),
    },
    BuiltinSkill {
        name: "corex-session-management",
        content: include_str!("../skills/corex-session-management/SKILL.md"),
    },
    BuiltinSkill {
        name: "corex-slash-commands",
        content: include_str!("../skills/corex-slash-commands/SKILL.md"),
    },
    BuiltinSkill {
        name: "corex-troubleshooting",
        content: include_str!("../skills/corex-troubleshooting/SKILL.md"),
    },
    BuiltinSkill {
        name: "corex-yolo-mode",
        content: include_str!("../skills/corex-yolo-mode/SKILL.md"),
    },
];

pub fn get_builtin_skill(name: &str) -> Option<&'static str> {
    BUILTIN_SKILLS.iter().find(|s| s.name == name).map(|s| s.content)
}

pub fn get_all_builtin_skills() -> &'static [BuiltinSkill] {
    BUILTIN_SKILLS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builtin_skills_present_and_non_empty() {
        assert_eq!(BUILTIN_SKILLS.len(), 8);
        assert!(get_builtin_skill("corex-about").is_some());
        assert!(get_builtin_skill("corex-about").unwrap().contains("sluisr"));
        assert!(get_builtin_skill("corex-fim").is_some());
        assert!(get_builtin_skill("non-existent").is_none());
    }
}
