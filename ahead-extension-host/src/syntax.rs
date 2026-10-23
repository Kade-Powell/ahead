use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
};

use anyhow::{Context, Result, anyhow, ensure};
use tree_sitter::{Parser, Query, QueryCursor, StreamingIterator, WasmStore};

use crate::host::{LanguageSource, LanguageSyntax};

const MAX_SOURCE_BYTES: usize = 8 * 1024 * 1024;
const MAX_CAPTURE_COUNT: usize = 100_000;
const MAX_CACHED_GRAMMARS: usize = 32;
const MAX_INJECTION_DEPTH: usize = 4;
const MAX_INJECTION_RANGES: usize = 512;
const MAX_INJECTED_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxCapture {
    pub start: usize,
    pub end: usize,
    pub scope: String,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct RuntimeKey {
    extension_directory: std::path::PathBuf,
    grammar_name: String,
    asset_digest: [u8; 32],
}

struct GrammarRuntime {
    parser: Parser,
    query: Query,
    injections: Option<Query>,
    _language: tree_sitter::Language,
}

struct PendingInjection {
    language: String,
    combined: bool,
    host_range: Option<(usize, usize)>,
    ranges: Vec<tree_sitter::Range>,
}

#[derive(Default)]
struct SyntaxBudget {
    captures: usize,
    injection_ranges: usize,
    injected_bytes: usize,
}

static ENGINE: OnceLock<tree_sitter::wasmtime::Engine> = OnceLock::new();
static GRAMMARS: OnceLock<Mutex<HashMap<RuntimeKey, Arc<Mutex<GrammarRuntime>>>>> =
    OnceLock::new();

pub(crate) fn captures(
    sources: &[LanguageSource],
    syntax: &LanguageSyntax,
    source: &str,
) -> Result<Vec<SyntaxCapture>> {
    let mut budget = SyntaxBudget::default();
    captures_inner(sources, syntax, source, None, 0, &mut budget)
}

fn captures_inner(
    sources: &[LanguageSource],
    syntax: &LanguageSyntax,
    source: &str,
    included_ranges: Option<&[tree_sitter::Range]>,
    depth: usize,
    budget: &mut SyntaxBudget,
) -> Result<Vec<SyntaxCapture>> {
    ensure!(
        source.len() <= MAX_SOURCE_BYTES,
        "source exceeds the extension syntax size limit"
    );
    if syntax.highlights.trim().is_empty() {
        return Ok(Vec::new());
    }

    let key = runtime_key(syntax);
    let runtime = {
        let cache = GRAMMARS.get_or_init(|| Mutex::new(HashMap::new()));
        let mut cache = cache
            .lock()
            .map_err(|_| anyhow!("extension grammar cache is unavailable"))?;
        if let Some(runtime) = cache.get(&key) {
            runtime.clone()
        } else {
            let runtime = Arc::new(Mutex::new(load_runtime(syntax)?));
            cache.retain(|cached_key, _| {
                cached_key.extension_directory != key.extension_directory
                    || cached_key.grammar_name != key.grammar_name
            });
            if cache.len() >= MAX_CACHED_GRAMMARS
                && let Some(oldest_key) = cache.keys().next().cloned()
            {
                cache.remove(&oldest_key);
            }
            cache.insert(key, runtime.clone());
            runtime
        }
    };

    let mut runtime = runtime
        .lock()
        .map_err(|_| anyhow!("extension grammar parser is unavailable"))?;
    if let Some(included_ranges) = included_ranges {
        runtime
            .parser
            .set_included_ranges(included_ranges)
            .context("setting extension injection ranges")?;
    }
    let parsed_tree = runtime.parser.parse(source, None);
    if included_ranges.is_some() {
        runtime
            .parser
            .set_included_ranges(&[])
            .context("resetting extension injection ranges")?;
    }
    let tree = parsed_tree.context("extension grammar cancelled parsing")?;
    let mut cursor = QueryCursor::new();
    cursor.set_match_limit(65_536);
    let mut captures =
        cursor.captures(&runtime.query, tree.root_node(), source.as_bytes());
    let capture_names = runtime.query.capture_names();
    let mut result = Vec::new();
    while budget.captures < MAX_CAPTURE_COUNT {
        let Some((query_match, capture_index)) = captures.next() else {
            break;
        };
        let Some(capture) = query_match.captures.get(*capture_index) else {
            continue;
        };
        let Some(scope) = capture_names.get(capture.index as usize) else {
            continue;
        };
        let node = capture.node;
        let start = node.start_byte();
        let end = node.end_byte();
        if start < end
            && end <= source.len()
            && source.is_char_boundary(start)
            && source.is_char_boundary(end)
        {
            result.push(SyntaxCapture {
                start,
                end,
                scope: (*scope).to_owned(),
            });
            budget.captures += 1;
        }
    }

    let mut pending_injections = Vec::<PendingInjection>::new();
    if depth < MAX_INJECTION_DEPTH
        && budget.injection_ranges < MAX_INJECTION_RANGES
        && let Some(query) = runtime.injections.as_ref()
        && let Some(content_capture) =
            query.capture_index_for_name("injection.content")
    {
        let language_capture = query.capture_index_for_name("injection.language");
        let host_capture = query.capture_index_for_name("injection.host");
        let mut cursor = QueryCursor::new();
        cursor.set_match_limit(65_536);
        let mut matches = cursor.matches(query, tree.root_node(), source.as_bytes());
        while budget.injection_ranges < MAX_INJECTION_RANGES {
            let Some(query_match) = matches.next() else {
                break;
            };
            let language = query
                .property_settings(query_match.pattern_index)
                .iter()
                .find(|property| property.key.as_ref() == "injection.language")
                .and_then(|property| property.value.as_deref())
                .map(str::to_owned)
                .or_else(|| {
                    let capture_index = language_capture?;
                    let capture = query_match
                        .captures
                        .iter()
                        .find(|capture| capture.index == capture_index)?;
                    let range = capture.node.byte_range();
                    source.get(range).map(str::to_owned)
                });
            let Some(language) = language
                .map(|language| {
                    language
                        .trim()
                        .rsplit('.')
                        .next()
                        .unwrap_or_default()
                        .to_owned()
                })
                .filter(|language| {
                    !language.is_empty()
                        && language.len() <= 128
                        && !language.chars().any(char::is_control)
                })
            else {
                continue;
            };
            let remaining_ranges =
                MAX_INJECTION_RANGES.saturating_sub(budget.injection_ranges);
            let ranges = query_match
                .captures
                .iter()
                .filter(|capture| capture.index == content_capture)
                .map(|capture| capture.node.range())
                .take(remaining_ranges)
                .filter(|range| {
                    range.start_byte < range.end_byte
                        && range.end_byte <= source.len()
                        && source.is_char_boundary(range.start_byte)
                        && source.is_char_boundary(range.end_byte)
                })
                .collect::<Vec<_>>();
            if ranges.is_empty() {
                continue;
            }
            let combined = query
                .property_settings(query_match.pattern_index)
                .iter()
                .any(|property| property.key.as_ref() == "injection.combined");
            let host_range = host_capture.and_then(|capture_index| {
                query_match
                    .captures
                    .iter()
                    .find(|capture| capture.index == capture_index)
                    .map(|capture| {
                        let range = capture.node.byte_range();
                        (range.start, range.end)
                    })
            });
            budget.injection_ranges += ranges.len();
            if combined || host_range.is_some() {
                if let Some(existing) =
                    pending_injections.iter_mut().find(|pending| {
                        pending.combined
                            && pending.language == language
                            && pending.host_range == host_range
                    })
                {
                    existing.ranges.extend(ranges);
                } else {
                    pending_injections.push(PendingInjection {
                        language,
                        combined,
                        host_range,
                        ranges,
                    });
                }
            } else {
                pending_injections.push(PendingInjection {
                    language,
                    combined,
                    host_range,
                    ranges,
                });
            }
        }
    }
    drop(runtime);

    for mut injection in pending_injections {
        if budget.captures >= MAX_CAPTURE_COUNT
            || budget.injected_bytes >= MAX_INJECTED_BYTES
        {
            break;
        }
        injection.ranges = normalize_included_ranges(injection.ranges);
        let injection_bytes = injection
            .ranges
            .iter()
            .map(|range| range.end_byte - range.start_byte)
            .sum::<usize>();
        if injection_bytes == 0
            || budget.injected_bytes.saturating_add(injection_bytes)
                > MAX_INJECTED_BYTES
        {
            continue;
        }
        let Some(injected_syntax) =
            super::host::language_syntax_for_name(sources, &injection.language)?
        else {
            continue;
        };
        budget.injected_bytes += injection_bytes;
        let injected_captures = captures_inner(
            sources,
            &injected_syntax,
            source,
            Some(&injection.ranges),
            depth + 1,
            budget,
        )?;
        result.extend(injected_captures);
    }
    Ok(result)
}

fn normalize_included_ranges(
    mut ranges: Vec<tree_sitter::Range>,
) -> Vec<tree_sitter::Range> {
    ranges.sort_unstable_by_key(|range| (range.start_byte, range.end_byte));
    let mut normalized: Vec<tree_sitter::Range> = Vec::with_capacity(ranges.len());
    for range in ranges {
        let Some(previous) = normalized.last_mut() else {
            normalized.push(range);
            continue;
        };
        if range.start_byte < previous.end_byte {
            if range.end_byte > previous.end_byte {
                previous.end_byte = range.end_byte;
                previous.end_point = range.end_point;
            }
        } else if range != *previous {
            normalized.push(range);
        }
    }
    normalized
}

fn runtime_key(syntax: &LanguageSyntax) -> RuntimeKey {
    RuntimeKey {
        extension_directory: syntax.extension_directory.clone(),
        grammar_name: syntax.grammar_name.clone(),
        asset_digest: syntax.asset_digest,
    }
}

fn load_runtime(syntax: &LanguageSyntax) -> Result<GrammarRuntime> {
    ensure!(
        !syntax.grammar_name.is_empty()
            && !syntax.grammar_name.as_bytes().contains(&0),
        "extension grammar name is invalid"
    );
    let engine = ENGINE.get_or_init(tree_sitter::wasmtime::Engine::default);
    let mut store =
        WasmStore::new(engine).context("creating extension grammar store")?;
    let language = store
        .load_language(&syntax.grammar_name, &syntax.wasm)
        .context("loading extension grammar WASM")?;
    let query = Query::new(&language, &syntax.highlights)
        .context("compiling extension highlights query")?;
    let injections = if syntax.injections.trim().is_empty() {
        None
    } else {
        Some(
            Query::new(&language, &syntax.injections)
                .context("compiling extension injections query")?,
        )
    };
    let mut parser = Parser::new();
    parser
        .set_wasm_store(store)
        .context("attaching extension grammar store")?;
    parser
        .set_language(&language)
        .context("configuring extension grammar parser")?;
    Ok(GrammarRuntime {
        parser,
        query,
        injections,
        _language: language,
    })
}
