//! The coding harnesses the standardizer knows. Supporting a new one is one row in
//! [`HARNESSES`], plus a transcript reader in the editor for conversation context.

/// How a harness invokes skills or commands inside a prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkillSyntax {
    /// `/deploy`: must stay at the very start of a draft to run.
    Slash,
    /// `$web-design-guidelines`: may appear anywhere.
    Dollar,
    None,
}

/// A model vendor whose prompt guide decides the prose format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    Anthropic,
    OpenAi,
}

#[derive(Debug)]
pub struct Harness {
    /// Wire id sent as `options.host`.
    pub id: &'static str,
    pub name: &'static str,
    pub skills: SkillSyntax,
    /// The vendor assumed when the session's model is unknown.
    pub family: Family,
}

pub const HARNESSES: &[Harness] = &[
    Harness {
        id: "claude",
        name: "Claude Code",
        skills: SkillSyntax::Slash,
        family: Family::Anthropic,
    },
    Harness {
        id: "codex",
        name: "Codex",
        skills: SkillSyntax::Dollar,
        family: Family::OpenAi,
    },
];

/// Finds a harness by id or display name, ignoring case (`codex`, `Claude Code`).
#[must_use]
pub fn find(value: &str) -> Option<&'static Harness> {
    let value = value.trim();
    HARNESSES.iter().find(|harness| {
        harness.id.eq_ignore_ascii_case(value) || harness.name.eq_ignore_ascii_case(value)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_harnesses_by_id_or_display_name() {
        assert_eq!(
            find("Claude Code").map(|harness| harness.id),
            Some("claude")
        );
        assert_eq!(find("CODEX").map(|harness| harness.name), Some("Codex"));
        assert!(find("unknown-agent").is_none());
    }
}
