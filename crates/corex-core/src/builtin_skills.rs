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
    BuiltinSkill {
        name: "corex-fleet",
        content: include_str!("../skills/corex-fleet/SKILL.md"),
    },
    BuiltinSkill {
        name: "corex-config",
        content: include_str!("../skills/corex-config/SKILL.md"),
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
        assert_eq!(BUILTIN_SKILLS.len(), 10);
        assert!(get_builtin_skill("corex-about").is_some());
        assert!(get_builtin_skill("corex-about").unwrap().contains("sluisr"));
        assert!(get_builtin_skill("corex-fim").is_some());
        assert!(get_builtin_skill("corex-fleet").is_some());
        assert!(get_builtin_skill("corex-config").is_some());
        assert!(get_builtin_skill("non-existent").is_none());
    }

    #[test]
    fn the_repo_agents_copy_matches_the_embedded_skills() {
        let mirror = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.agents/skills");
        let Ok(entries) = std::fs::read_dir(&mirror) else { return };
        let mut mirrored: Vec<String> = entries.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        mirrored.sort();
        let mut embedded: Vec<String> = BUILTIN_SKILLS.iter().map(|s| s.name.to_string()).collect();
        embedded.sort();
        assert_eq!(mirrored, embedded, ".agents/skills must list the same skills as BUILTIN_SKILLS");
        for skill in BUILTIN_SKILLS {
            let copy = std::fs::read_to_string(mirror.join(skill.name).join("SKILL.md")).unwrap_or_default();
            assert!(copy == skill.content, ".agents/skills/{}/SKILL.md differs from crates/corex-core/skills", skill.name);
        }
    }
}
