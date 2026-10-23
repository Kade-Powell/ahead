use crate::McpServerIdentity;
use crate::McpServerRequirement;
use crate::McpServerValueMatcher;
use crate::mcp_types::McpServerConfig;
use regex_lite::Regex;

fn compile_full_regex(expression: &str) -> Result<Regex, String> {
    Regex::new(&format!(r"\A(?:{expression})\z")).map_err(|err| {
        format!("regex `{expression}` cannot be used for full-value matching: {err}")
    })
}

fn validate_value_matcher(matcher: &McpServerValueMatcher) -> Result<(), String> {
    let McpServerValueMatcher::Regex { expression } = matcher else {
        return Ok(());
    };

    Regex::new(expression).map_err(|err| format!("invalid regex `{expression}`: {err}"))?;
    compile_full_regex(expression).map(|_| ())
}

fn matches_value(matcher: &McpServerValueMatcher, candidate: &str) -> bool {
    match matcher {
        McpServerValueMatcher::Exact { value } => candidate == value,
        McpServerValueMatcher::Prefix { value } => candidate.starts_with(value),
        McpServerValueMatcher::Regex { expression } => compile_full_regex(expression)
            .ok()
            .is_some_and(|regex| regex.is_match(candidate)),
    }
}

pub(crate) fn validate_mcp_server_requirement(
    requirement: &McpServerRequirement,
) -> Result<(), String> {
    match requirement {
        McpServerRequirement::Identity { .. } => Ok(()),
        McpServerRequirement::Command(matcher) => {
            for (index, arg) in matcher.args.iter().enumerate() {
                validate_value_matcher(arg)
                    .map_err(|err| format!("invalid argument matcher at index {index}: {err}"))?;
            }
            Ok(())
        }
    }
}

impl McpServerConfig {
    pub fn matches_requirement(&self, requirement: &McpServerRequirement) -> bool {
        let crate::mcp_types::McpServerTransportConfig::Stdio { command, args, .. } =
            &self.transport;
        match requirement {
            McpServerRequirement::Identity {
                identity:
                    McpServerIdentity::Command {
                        command: want_command,
                    },
            } => command == want_command,
            McpServerRequirement::Command(matcher) => {
                matcher.executable == *command
                    && matcher.args.len() == args.len()
                    && matcher
                        .args
                        .iter()
                        .zip(args)
                        .all(|(matcher, arg)| matches_value(matcher, arg))
            }
        }
    }
}

#[cfg(test)]
#[path = "mcp_requirements_tests.rs"]
mod tests;
