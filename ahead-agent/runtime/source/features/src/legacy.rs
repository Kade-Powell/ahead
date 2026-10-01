use crate::Feature;
use crate::Features;
use tracing::info;

#[derive(Clone, Copy)]
struct Alias {
    legacy_key: &'static str,
    feature: Feature,
}

const ALIASES: &[Alias] = &[
    Alias {
        legacy_key: "enable_experimental_windows_sandbox",
        feature: Feature::WindowsSandbox,
    },
    Alias {
        legacy_key: "experimental_use_unified_exec_tool",
        feature: Feature::UnifiedExec,
    },
    Alias {
        legacy_key: "request_permissions",
        feature: Feature::ExecPermissionApprovals,
    },
    Alias {
        legacy_key: "web_search",
        feature: Feature::WebSearchRequest,
    },
    Alias {
        legacy_key: "imagegenext",
        feature: Feature::ImageGeneration,
    },
    Alias {
        legacy_key: "collab",
        feature: Feature::Collab,
    },
    Alias {
        legacy_key: "memory_tool",
        feature: Feature::MemoryTool,
    },
    Alias {
        legacy_key: "telepathy",
        feature: Feature::Chronicle,
    },
    Alias {
        legacy_key: "codex_hooks",
        feature: Feature::CodexHooks,
    },
];

const IGNORED_FEATURE_KEYS: &[&str] = &[
    "guardian_reuse_parent_compaction",
    "guardian_enhanced_node_repl_transcripts",
    "guardian_node_repl_transcript_images",
    "guardian_ext",
];

pub fn legacy_feature_keys() -> impl Iterator<Item = &'static str> {
    ALIASES.iter().map(|alias| alias.legacy_key)
}

/// Returns removed feature keys that remain accepted but never enable behavior.
pub fn legacy_ignored_feature_keys() -> impl Iterator<Item = &'static str> {
    IGNORED_FEATURE_KEYS.iter().copied()
}

pub(crate) fn is_legacy_ignored_feature_key(key: &str) -> bool {
    IGNORED_FEATURE_KEYS.contains(&key)
}

pub(crate) fn feature_for_key(key: &str) -> Option<Feature> {
    ALIASES
        .iter()
        .find(|alias| alias.legacy_key == key)
        .map(|alias| {
            log_alias(alias.legacy_key, alias.feature);
            alias.feature
        })
}

#[derive(Debug, Default)]
pub(crate) struct LegacyFeatureToggles {
    pub experimental_use_unified_exec_tool: Option<bool>,
}

impl LegacyFeatureToggles {
    pub fn apply(self, features: &mut Features) {
        set_if_some(
            features,
            Feature::UnifiedExec,
            self.experimental_use_unified_exec_tool,
            "experimental_use_unified_exec_tool",
        );
    }
}

fn set_if_some(
    features: &mut Features,
    feature: Feature,
    maybe_value: Option<bool>,
    alias_key: &'static str,
) {
    if let Some(enabled) = maybe_value {
        set_feature(features, feature, enabled);
        log_alias(alias_key, feature);
        features.record_legacy_usage(alias_key, feature);
    }
}

fn set_feature(features: &mut Features, feature: Feature, enabled: bool) {
    if enabled {
        features.enable(feature);
    } else {
        features.disable(feature);
    }
}

fn log_alias(alias: &str, feature: Feature) {
    let canonical = feature.key();
    if alias == canonical {
        return;
    }
    info!(
        %alias,
        canonical,
        "legacy feature toggle detected; prefer `[features].{canonical}`"
    );
}
