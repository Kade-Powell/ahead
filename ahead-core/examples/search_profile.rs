use ahead_core::search::{
    FileSearchOptions, SearchScope, WorkspaceFileIndex, search_paths,
};
use std::{
    collections::HashSet,
    error::Error,
    fmt::Write as _,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

fn main() -> Result<(), Box<dyn Error>> {
    const GROUP_COUNT: usize = 100;
    const FILES_PER_GROUP: usize = 100;
    const SEARCH_PASSES: usize = 3;
    const PATH_INDEX_PASSES: usize = 5;
    const CONCURRENT_SEARCH_COUNTS: [usize; 4] = [1, 2, 4, 8];

    let workspace = tempfile::tempdir()?;
    let filler = "    let value = \"ordinary search fixture content\";\n".repeat(32);
    for group in 0..GROUP_COUNT {
        let directory = workspace.path().join(format!("src/group-{group:03}"));
        std::fs::create_dir_all(&directory)?;
        for file in 0..FILES_PER_GROUP {
            let index = group * FILES_PER_GROUP + file;
            let mut content = String::with_capacity(filler.len() + 128);
            writeln!(content, "fn file_{index}() {{")?;
            content.push_str(&filler);
            if index.is_multiple_of(1_000) {
                content.push_str("    // AHEAD_SEARCH_PROFILE_NEEDLE\n");
            }
            content.push_str("}\n");
            std::fs::write(directory.join(format!("file_{file:03}.rs")), content)?;
        }
    }

    let workspace_path = workspace.path();
    let expected_files = GROUP_COUNT * FILES_PER_GROUP;
    let mut serial_index_elapsed = Vec::with_capacity(PATH_INDEX_PASSES);
    let mut parallel_index_elapsed = Vec::with_capacity(PATH_INDEX_PASSES);
    let mut paths = None;
    for pass in 0..PATH_INDEX_PASSES {
        let (
            serial_duration,
            serial_paths,
            visible_entries,
            parallel_duration,
            indexed_paths,
        ) = if pass % 2 == 0 {
            let (serial_duration, serial_paths, visible_entries) =
                measure_serial_index(workspace.path());
            let (parallel_duration, indexed_paths) =
                measure_indexed_paths(workspace.path());
            (
                serial_duration,
                serial_paths,
                visible_entries,
                parallel_duration,
                indexed_paths,
            )
        } else {
            let (parallel_duration, indexed_paths) =
                measure_indexed_paths(workspace.path());
            let (serial_duration, serial_paths, visible_entries) =
                measure_serial_index(workspace.path());
            (
                serial_duration,
                serial_paths,
                visible_entries,
                parallel_duration,
                indexed_paths,
            )
        };
        if serial_paths.len() != expected_files
            || indexed_paths.len() != expected_files
            || serial_paths.as_slice() != indexed_paths.as_slice()
            || visible_entries < expected_files
        {
            return Err(format!(
                "index parity failed: expected {expected_files} matching files and visible entries, serial found {} files / {visible_entries} entries and parallel found {} files",
                serial_paths.len(),
                indexed_paths.len(),
            )
            .into());
        }
        serial_index_elapsed.push(serial_duration);
        parallel_index_elapsed.push(parallel_duration);
        paths = Some(indexed_paths);
    }
    let paths = paths.ok_or("path index profile produced no snapshots")?;
    let options = FileSearchOptions {
        pattern: "AHEAD_SEARCH_PROFILE_NEEDLE".into(),
        case_sensitive: true,
        whole_word: false,
        is_regex: false,
        max_results: 1_000,
    };
    let mut elapsed = Vec::<Duration>::with_capacity(SEARCH_PASSES);
    let mut match_count = 0;
    for _ in 0..SEARCH_PASSES {
        let search_started = Instant::now();
        let results = search_paths(
            SearchScope::Workspace(workspace.path()),
            paths.as_ref().clone(),
            &options,
            || true,
        )?;
        elapsed.push(search_started.elapsed());
        match_count = results.iter().map(|(_, matches)| matches.len()).sum();
    }
    if match_count != 10 {
        return Err(format!("expected 10 matches, found {match_count}").into());
    }

    let no_hit_options = FileSearchOptions {
        pattern: "AHEAD_SEARCH_PROFILE_ABSENT".into(),
        case_sensitive: true,
        whole_word: false,
        is_regex: false,
        max_results: 1_000,
    };
    let mut concurrent_elapsed = Vec::new();
    for search_count in CONCURRENT_SEARCH_COUNTS {
        let mut passes = Vec::with_capacity(SEARCH_PASSES);
        for _ in 0..SEARCH_PASSES {
            let started = Instant::now();
            let match_count = std::thread::scope(|scope| {
                let searches = (0..search_count)
                    .map(|_| {
                        let paths = paths.as_ref().clone();
                        let no_hit_options = &no_hit_options;
                        scope.spawn(move || {
                            search_paths(
                                SearchScope::Workspace(workspace_path),
                                paths,
                                no_hit_options,
                                || true,
                            )
                        })
                    })
                    .collect::<Vec<_>>();
                let mut match_count = 0;
                for search in searches {
                    let results = search.join().map_err(|_| {
                        io::Error::other("concurrent search panicked")
                    })??;
                    match_count += results
                        .iter()
                        .map(|(_, matches)| matches.len())
                        .sum::<usize>();
                }
                Ok::<_, Box<dyn Error>>(match_count)
            })?;
            if match_count != 0 {
                return Err(
                    format!("no-hit search returned {match_count} matches").into()
                );
            }
            passes.push(started.elapsed());
        }
        let mut sorted_passes = passes.clone();
        sorted_passes.sort_unstable();
        let median = sorted_passes
            .get(SEARCH_PASSES / 2)
            .ok_or("concurrent search profile produced no timings")?;
        concurrent_elapsed.push((search_count, passes, *median));
    }

    let mut sorted_elapsed = elapsed.clone();
    sorted_elapsed.sort_unstable();
    let median = sorted_elapsed
        .get(SEARCH_PASSES / 2)
        .ok_or("search profile produced no timings")?;
    let mut stdout = io::stdout().lock();
    writeln!(stdout, "files={} matches={match_count}", paths.len())?;
    let median_path_ms = |durations: &[Duration]| {
        let mut sorted = durations.to_vec();
        sorted.sort_unstable();
        sorted.get(sorted.len() / 2).map(Duration::as_millis)
    };
    writeln!(
        stdout,
        "serial_index_ms={:?} median_serial_ms={} parallel_index_ms={:?} median_parallel_ms={}",
        serial_index_elapsed
            .iter()
            .map(Duration::as_millis)
            .collect::<Vec<_>>(),
        median_path_ms(&serial_index_elapsed).ok_or("no serial index samples")?,
        parallel_index_elapsed
            .iter()
            .map(Duration::as_millis)
            .collect::<Vec<_>>(),
        median_path_ms(&parallel_index_elapsed)
            .ok_or("no parallel index samples")?,
    )?;
    writeln!(
        stdout,
        "search_pass_ms_in_order={:?} median_search_ms={}",
        elapsed.iter().map(Duration::as_millis).collect::<Vec<_>>(),
        median.as_millis()
    )?;
    for (search_count, passes, median) in concurrent_elapsed {
        writeln!(
            stdout,
            "concurrent_no_hit_searches={search_count} batch_ms={:?} median_batch_ms={}",
            passes.iter().map(Duration::as_millis).collect::<Vec<_>>(),
            median.as_millis()
        )?;
    }
    Ok(())
}

fn measure_serial_index(workspace: &Path) -> (Duration, Vec<PathBuf>, usize) {
    let started = Instant::now();
    let mut walker = ignore::WalkBuilder::new(workspace);
    walker.require_git(false);
    walker.sort_by_file_name(|left, right| left.cmp(right));
    let mut files = Vec::new();
    let mut visible_entries = HashSet::new();
    for entry in walker.build().filter_map(Result::ok) {
        let Some(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.into_path();
        if kind.is_file() {
            files.push(path.clone());
        }
        if kind.is_file() || kind.is_dir() {
            visible_entries.insert(path);
        }
    }
    (started.elapsed(), files, visible_entries.len())
}

fn measure_indexed_paths(workspace: &Path) -> (Duration, Arc<Vec<PathBuf>>) {
    let started = Instant::now();
    let paths = WorkspaceFileIndex::new(workspace.to_path_buf()).snapshot();
    (started.elapsed(), paths)
}
