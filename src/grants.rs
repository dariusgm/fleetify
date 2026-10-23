//! What the user grants, read from `credentials.toml` next to the tokens. A recipe can
//! only ask for these (`needs`, `policy`, `ci.merge`): nothing in a recipe grants
//! anything, so a shared recipe cannot give itself more. See DECISIONS.md, "Remove the
//! config.toml layer".

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::credentials::glob_match;

/// `[permissions]`: what fleetify may grant to an agent run. A recipe's `needs` are
/// checked against these; a run that asks for more than is granted is refused.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Permissions {
    /// Commands the agent may run, as written in `needs.commands`.
    #[serde(default)]
    pub commands: Vec<String>,
    /// Web access for the agent.
    #[serde(default)]
    pub web: bool,
    /// Extra directories the agent may write to, as written in `needs.extra_directories`.
    #[serde(default)]
    pub extra_directories: Vec<String>,
    /// Lets a recipe set `policy.require_checked_agent = false`.
    #[serde(default)]
    pub unchecked_agents: bool,
    /// Per agent, keyed by agent name.
    #[serde(default)]
    pub agent: BTreeMap<String, AgentPermissions>,
}

/// `[permissions.agent.<name>]`: grants for one agent.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AgentPermissions {
    /// Use the agent CLI's blanket "allow everything" option, for a recipe that asks for
    /// `needs.unrestricted`. At the user's own risk.
    #[serde(default)]
    pub unrestricted: bool,
}

/// `[[merge_policy]]`: merge permissions for the repositories whose `<host>/<path>`
/// matches `pattern`. Rules are checked in order and the first match decides. A
/// repository without a matching rule is never merged without a CI gate.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MergeRule {
    /// Glob on `<host>/<path>`, e.g. `github.com/owner/*`.
    #[serde(rename = "match")]
    pub pattern: String,
    /// Merge with `--merge` without a CI gate. Only the user can grant this, here.
    #[serde(default)]
    pub merge_without_ci: bool,
    /// Delete the source branch after a merge. The recipe must ask for it too.
    #[serde(default)]
    pub delete_branch: bool,
}

/// The user's grants, as a whole.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Grants {
    pub permissions: Permissions,
    pub merge_policy: Vec<MergeRule>,
}

impl Grants {
    /// The first merge rule whose pattern matches `key` (`<host>/<path>`).
    pub fn merge_rule(&self, key: &str) -> Option<&MergeRule> {
        self.merge_policy
            .iter()
            .find(|rule| glob_match(&rule.pattern, key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_grants_grant_nothing() {
        let grants = Grants::default();
        assert!(grants.permissions.commands.is_empty());
        assert!(!grants.permissions.web);
        assert!(!grants.permissions.unchecked_agents);
        assert!(grants.permissions.agent.is_empty());
        assert!(grants.merge_rule("github.com/owner/repo").is_none());
    }

    #[test]
    fn merge_rules_match_repositories_and_the_first_match_wins() {
        let grants = Grants {
            merge_policy: toml::from_str::<Wrapper>(
                r#"
                [[merge_policy]]
                match = "github.com/owner/docs"
                merge_without_ci = true
                delete_branch = true

                [[merge_policy]]
                match = "github.com/owner/*"
                merge_without_ci = false
                "#,
            )
            .unwrap()
            .merge_policy,
            ..Grants::default()
        };
        let docs = grants.merge_rule("github.com/owner/docs").unwrap();
        assert!(docs.merge_without_ci && docs.delete_branch);
        let other = grants.merge_rule("github.com/owner/service").unwrap();
        assert!(!other.merge_without_ci);
        assert!(grants.merge_rule("gitlab.com/owner/service").is_none());
    }

    #[derive(Deserialize)]
    struct Wrapper {
        #[serde(default)]
        merge_policy: Vec<MergeRule>,
    }
}
