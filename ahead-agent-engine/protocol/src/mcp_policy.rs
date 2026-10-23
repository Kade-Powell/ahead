use serde::Deserialize;
use std::collections::BTreeMap;

/// Additional managed MCP restrictions supplied by an environment owner.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnvironmentMcpPolicy {
    pub servers: Option<BTreeMap<String, McpServerRequirement>>,
}

#[derive(Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(untagged)]
pub enum McpServerIdentity {
    Command { command: String },
}

/// String matching operations available to managed MCP server matchers.
#[derive(Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "match", rename_all = "snake_case", deny_unknown_fields)]
pub enum McpServerValueMatcher {
    Exact { value: String },
    Prefix { value: String },
    Regex { expression: String },
}

#[derive(Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct McpServerCommandMatcher {
    pub executable: String,
    pub args: Vec<McpServerValueMatcher>,
}

#[derive(Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct RawMcpServerCommandIdentity {
    command: McpServerCommandMatcher,
}

/// A requirement for one named MCP server.
///
/// The `Identity` variant preserves the released exact-match contract. The
/// command matcher is the normalized matcher-based form accepted under the
/// `identity` key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpServerRequirement {
    Identity { identity: McpServerIdentity },
    Command(McpServerCommandMatcher),
}

#[derive(Deserialize)]
struct RawMcpServerRequirement {
    identity: RawMcpServerIdentity,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawMcpServerIdentity {
    Exact(McpServerIdentity),
    Command(RawMcpServerCommandIdentity),
}

impl<'de> Deserialize<'de> for McpServerRequirement {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let RawMcpServerRequirement { identity } =
            RawMcpServerRequirement::deserialize(deserializer)?;
        match identity {
            RawMcpServerIdentity::Exact(identity) => Ok(Self::Identity { identity }),
            RawMcpServerIdentity::Command(matcher) => Ok(Self::Command(matcher.command)),
        }
    }
}
