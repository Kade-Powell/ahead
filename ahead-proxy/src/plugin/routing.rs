use std::{collections::BTreeMap, path::Path};

use anyhow::{Context, Result, ensure};
use serde::Deserialize;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct LanguageServerRouting {
    languages: BTreeMap<String, LanguageRouting>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
struct LanguageRouting {
    language_servers: Option<Vec<String>>,
    requests: BTreeMap<String, String>,
}

fn language_key(language: &str) -> String {
    super::catalog::builtin_protocol_language_id(language)
        .unwrap_or(language)
        .to_ascii_lowercase()
}

impl LanguageServerRouting {
    pub fn load(workspace: Option<&Path>, user_home: Option<&Path>) -> Result<Self> {
        let mut routing = Self::default();
        let mut layers = Vec::new();
        if let Some(home) = user_home {
            layers.push((home, "settings.toml"));
        }
        if let Some(workspace) = workspace {
            layers.extend([
                (workspace, "config.toml"),
                (workspace, "config.local.toml"),
                (workspace, "settings.toml"),
            ]);
        }
        for (root, filename) in layers {
            if let Some(content) = ahead_core::config::read_ahead_config(
                root, filename,
            )
            .with_context(|| format!("reading language routing from {filename}"))?
            {
                routing.apply_layer(&content, filename)?;
            }
        }
        Ok(routing)
    }

    fn apply_layer(&mut self, content: &str, source: &str) -> Result<()> {
        let config = content.parse::<toml::Table>().map_err(|error| {
            anyhow::anyhow!("parsing {source}: {}", error.message())
        })?;
        let Some(languages) = config.get("languages") else {
            return Ok(());
        };
        let languages = languages
            .as_table()
            .context("[languages] must be a table")?;
        let mut keys = std::collections::HashSet::new();
        for (language, value) in languages {
            let key = language_key(language);
            ensure!(
                keys.insert(key.clone()),
                "{source} defines the same language more than once"
            );
            let layer: LanguageRouting = value.clone().try_into().map_err(|_| {
                anyhow::anyhow!(
                    "invalid language routing in {source} [languages.{language}]"
                )
            })?;
            if let Some(servers) = &layer.language_servers {
                let mut names = std::collections::HashSet::new();
                for name in servers {
                    ensure!(
                        !name.trim_start_matches('!').is_empty()
                            && name.trim() == name,
                        "language server names must not be empty or contain surrounding whitespace"
                    );
                    ensure!(
                        names.insert(name),
                        "duplicate language server entry {name}"
                    );
                }
            }
            for (method, name) in &layer.requests {
                ensure!(
                    method.contains('/')
                        && !name.is_empty()
                        && name.trim() == name
                        && !name.starts_with('!')
                        && name != "...",
                    "language request routes require an LSP method and a server name"
                );
            }
            let existing = self.languages.entry(key).or_default();
            if layer.language_servers.is_some() {
                existing.language_servers = layer.language_servers;
            }
            existing.requests.extend(layer.requests);
        }
        Ok(())
    }

    pub fn servers(
        &self,
        language: &str,
        available: &[String],
        defaults: &[String],
        opt_in: &[String],
    ) -> Result<Vec<String>> {
        let configured = self.languages.get(&language_key(language));
        let Some(entries) =
            configured.and_then(|routing| routing.language_servers.as_ref())
        else {
            return Ok(defaults.to_vec());
        };
        let excluded = entries
            .iter()
            .filter_map(|entry| entry.strip_prefix('!'))
            .collect::<Vec<_>>();
        let mut ordered = Vec::new();
        for entry in entries {
            if entry.starts_with('!') {
                continue;
            }
            let names = if entry == "..." {
                available
                    .iter()
                    .filter(|name| !opt_in.contains(name))
                    .collect::<Vec<_>>()
            } else {
                ensure!(
                    available.contains(entry),
                    "Unknown language server {entry} for {language}"
                );
                vec![entry]
            };
            for name in names {
                if !excluded.contains(&name.as_str()) && !ordered.contains(name) {
                    ordered.push(name.clone());
                }
            }
        }
        Ok(ordered)
    }

    pub fn request_server(&self, language: &str, method: &str) -> Option<&str> {
        self.languages
            .get(&language_key(language))?
            .requests
            .get(method)
            .map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(values: &[&str]) -> Vec<String> {
        values.iter().map(|name| (*name).into()).collect()
    }

    #[test]
    fn ordered_servers_support_exclusions_explicit_lists_and_language_aliases()
    -> Result<()> {
        let mut routing = LanguageServerRouting::default();
        let available = names(&["pyright", "basedpyright-langserver", "ruff"]);
        let defaults = names(&["pyright", "ruff"]);
        assert_eq!(
            routing.servers("python", &available, &defaults, &[])?,
            defaults
        );
        routing.apply_layer(
            "[languages.Python]\nlanguage_servers = ['ruff', '!pyright', '...']\n",
            "test",
        )?;
        assert_eq!(
            routing.servers("python", &available, &defaults, &[])?,
            names(&["ruff", "basedpyright-langserver"])
        );
        routing.apply_layer(
            "[languages.python]\nlanguage_servers = ['basedpyright-langserver']\n",
            "test",
        )?;
        assert_eq!(
            routing.servers("Python", &available, &defaults, &[])?,
            names(&["basedpyright-langserver"])
        );
        routing
            .apply_layer("[languages.Python]\nlanguage_servers = []\n", "test")?;
        assert!(
            routing
                .servers("python", &available, &defaults, &[])?
                .is_empty()
        );
        routing.apply_layer(
            "[languages.TSX.requests]\n\"textDocument/hover\" = 'vtsls'\n",
            "test",
        )?;
        assert_eq!(
            routing.request_server("typescriptreact", "textDocument/hover"),
            Some("vtsls")
        );
        Ok(())
    }

    #[test]
    fn wildcard_excludes_opt_in_servers_but_explicit_id_enables_them() -> Result<()>
    {
        let mut routing = LanguageServerRouting::default();
        let available = names(&["html-server"]);
        let defaults = Vec::new();
        let opt_in = names(&["html-server"]);
        routing
            .apply_layer("[languages.HTML]\nlanguage_servers = ['...']\n", "test")?;
        assert!(
            routing
                .servers("HTML", &available, &defaults, &opt_in)?
                .is_empty()
        );
        routing.apply_layer(
            "[languages.HTML]\nlanguage_servers = ['html-server']\n",
            "test",
        )?;
        assert_eq!(
            routing.servers("html", &available, &defaults, &opt_in)?,
            available
        );
        Ok(())
    }

    #[test]
    fn layers_merge_requests_override_order_and_reject_invalid_configuration()
    -> Result<()> {
        let root = tempfile::tempdir()?;
        let user = root.path().join("user");
        let project = root.path().join("project");
        for directory in [&user, &project] {
            std::fs::create_dir_all(directory.join(".ahead"))?;
        }
        std::fs::write(
            user.join(".ahead/settings.toml"),
            "[languages.Python]\nlanguage_servers = ['ruff', '...']\n[languages.Python.requests]\n\"textDocument/formatting\" = 'ruff'\n",
        )?;
        std::fs::write(
            project.join(".ahead/config.toml"),
            "[languages.python]\nlanguage_servers = ['basedpyright-langserver', 'ruff']\n[languages.python.requests]\n\"textDocument/hover\" = 'basedpyright-langserver'\n",
        )?;
        std::fs::write(
            project.join(".ahead/settings.toml"),
            "[languages.Python.requests]\n\"textDocument/hover\" = 'ruff'\n",
        )?;
        let routing = LanguageServerRouting::load(Some(&project), Some(&user))?;
        assert_eq!(
            routing.request_server("python", "textDocument/formatting"),
            Some("ruff")
        );
        assert_eq!(
            routing.request_server("python", "textDocument/hover"),
            Some("ruff")
        );
        assert_eq!(
            routing.servers(
                "python",
                &names(&["basedpyright-langserver", "ruff"]),
                &[],
                &[],
            )?,
            names(&["basedpyright-langserver", "ruff"])
        );
        for invalid in [
            "languages = 1",
            "[languages.python]\nlanguage_servers = ['']",
            "[languages.python]\nlanguage_servers = ['ruff', 'ruff']",
            "[languages.python.requests]\nhover = 'ruff'",
            "[languages.python.requests]\n\"textDocument/hover\" = '...'",
            "[languages.python]\n[languages.Python]",
        ] {
            assert!(
                LanguageServerRouting::default()
                    .apply_layer(invalid, "test")
                    .is_err(),
                "accepted {invalid}"
            );
        }
        let mut routing = LanguageServerRouting::default();
        routing.apply_layer(
            "[languages.python]\nlanguage_servers = ['missing']",
            "test",
        )?;
        assert!(
            routing
                .servers("python", &names(&["ruff"]), &[], &[])
                .is_err()
        );
        Ok(())
    }
}
