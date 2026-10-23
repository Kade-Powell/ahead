//! Editor-owned workspace search shared by the proxy and agent harness.
//!
//! The shape follows Zed's `project::search` boundary: the editor owns the
//! worktree view and filtering, while callers only receive bounded matches.
//! The native agent must not grow a second filesystem-search implementation.

use std::{
    borrow::Cow,
    collections::{BTreeMap, HashMap, HashSet},
    fmt,
    fs::File,
    io::{self, Read, Seek},
    ops::Range,
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    sync::mpsc::{self, RecvTimeoutError},
    sync::{Arc, Condvar, Mutex, OnceLock},
    time::Duration,
};

use chardetng::EncodingDetector;
use encoding_rs::Encoding;
use fancy_regex::{
    NoExpand as FancyNoExpand, Regex as FancyRegex,
    RegexBuilder as FancyRegexBuilder,
};
use globset::{Glob, GlobSet, GlobSetBuilder};
use grep_matcher::{LineTerminator, Matcher, NoError};
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::{BinaryDetection, Searcher, SearcherBuilder, Sink, sinks::UTF8};
use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher as FuzzyMatcher, Utf32Str};
use rayon::{ThreadPool, ThreadPoolBuilder};
use regex::{NoExpand, Regex, RegexBuilder};

const SEARCH_PREFIX_BYTES: usize = 1024;
pub const MAX_SEARCH_RESULT_RANGES: usize = 10_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSearchOptions {
    pub pattern: String,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub is_regex: bool,
    pub max_results: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSearchMatch {
    /// Zero-based line containing the start of the match.
    pub line: usize,
    /// Zero-based byte column in `line`, even when `line_content` is clipped.
    pub start: usize,
    /// Zero-based line containing the exclusive end of the match.
    pub end_line: usize,
    /// Exclusive byte column in `end_line`.
    pub end: usize,
    pub line_content: String,
    /// Match range in the UTF-8 preview line, clipped to its visible first line.
    pub preview_match: Range<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileSearchReplacement {
    pub path: PathBuf,
    pub original_text: String,
    pub replacement_text: String,
    pub match_count: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RankedPathMatch {
    pub path: PathBuf,
    pub relative_path: String,
    pub score: f64,
    /// Unicode scalar-value positions in `relative_path` for picker highlighting.
    pub positions: Vec<usize>,
    pub distance_to_relative_directory: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileSearchError {
    InvalidPattern(String),
    InvalidGlob(String),
    InvalidWorkspace(String),
    TooManyMatches,
    RegexExecution(String),
    Cancelled,
    WorkerFailed,
}

impl fmt::Display for FileSearchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPattern(error) => {
                write!(formatter, "can't build matcher: {error}")
            }
            Self::InvalidGlob(error) => {
                write!(formatter, "invalid path glob: {error}")
            }
            Self::InvalidWorkspace(error) => {
                write!(formatter, "can't open search workspace: {error}")
            }
            Self::TooManyMatches => write!(
                formatter,
                "search replacement exceeded the {MAX_SEARCH_RESULT_RANGES}-match safety limit; narrow the search"
            ),
            Self::RegexExecution(error) => {
                write!(formatter, "regular expression search failed: {error}")
            }
            Self::Cancelled => formatter.write_str("expired search job"),
            Self::WorkerFailed => {
                formatter.write_str("workspace search worker stopped unexpectedly")
            }
        }
    }
}

impl std::error::Error for FileSearchError {}

struct Utf8ValidatingReader<'a, R> {
    inner: R,
    incomplete: Vec<u8>,
    validation: Vec<u8>,
    prefix: &'a mut Vec<u8>,
    prefix_offset: usize,
    initialized: bool,
}

impl<'a, R: Read> Utf8ValidatingReader<'a, R> {
    fn new(inner: R, prefix: &'a mut Vec<u8>) -> Self {
        prefix.clear();
        Self {
            inner,
            incomplete: Vec::new(),
            validation: Vec::new(),
            prefix,
            prefix_offset: 0,
            initialized: false,
        }
    }

    fn initialize(&mut self) -> io::Result<()> {
        if self.initialized {
            return Ok(());
        }
        let mut buffer = [0; SEARCH_PREFIX_BYTES];
        while self.prefix.len() < SEARCH_PREFIX_BYTES {
            let remaining = SEARCH_PREFIX_BYTES - self.prefix.len();
            let bytes_read = self.inner.read(&mut buffer[..remaining])?;
            if bytes_read == 0 {
                break;
            }
            self.prefix.extend_from_slice(&buffer[..bytes_read]);
        }
        match classify_search_bytes(&self.prefix) {
            SearchByteClass::Utf8 => self.initialized = true,
            SearchByteClass::Encoded => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "encoded text requires decoding before search",
                ));
            }
            SearchByteClass::Binary => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "binary content is not searchable text",
                ));
            }
        }
        Ok(())
    }
}

impl<R: Read> Read for Utf8ValidatingReader<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        self.initialize()?;
        let bytes_read = if self.prefix_offset < self.prefix.len() {
            let bytes_read =
                buffer.len().min(self.prefix.len() - self.prefix_offset);
            buffer[..bytes_read].copy_from_slice(
                &self.prefix[self.prefix_offset..self.prefix_offset + bytes_read],
            );
            self.prefix_offset += bytes_read;
            bytes_read
        } else {
            let bytes_read = self.inner.read(buffer)?;
            if bytes_read > 0 {
                let chunk = &buffer[..bytes_read];
                if chunk.contains(&0)
                    || (bytes_read >= 128 && has_binary_control_bytes(chunk))
                {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "binary content is not searchable text",
                    ));
                }
            }
            bytes_read
        };
        if bytes_read == 0 {
            if self.incomplete.is_empty() {
                return Ok(0);
            }
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "incomplete UTF-8 sequence at end of search path",
            ));
        }

        let has_incomplete_prefix = !self.incomplete.is_empty();
        if has_incomplete_prefix {
            self.validation.clear();
            self.validation.extend_from_slice(&self.incomplete);
            self.validation.extend_from_slice(&buffer[..bytes_read]);
        }
        let bytes_to_validate = if has_incomplete_prefix {
            &self.validation[..]
        } else {
            &buffer[..bytes_read]
        };
        match std::str::from_utf8(bytes_to_validate) {
            Ok(_) => self.incomplete.clear(),
            Err(error) if error.error_len().is_none() => {
                self.incomplete.clear();
                self.incomplete
                    .extend_from_slice(&bytes_to_validate[error.valid_up_to()..]);
            }
            Err(error) => {
                return Err(io::Error::new(io::ErrorKind::InvalidData, error));
            }
        }
        Ok(bytes_read)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SearchByteClass {
    Utf8,
    Encoded,
    Binary,
}

/// Reuse the search classifier when deciding whether to offer a text editor.
pub fn is_binary_content(bytes: &[u8]) -> bool {
    matches!(classify_search_bytes(bytes), SearchByteClass::Binary)
}

/// Classify a bounded prefix before grep's NUL shortcut can drop text encodings.
/// Byte-content heuristics are adapted from Zed's `language::file_content`
/// (`analyze_byte_content` and `is_plausible_utf16_text`), Apache-2.0,
/// Copyright 2022-2025 Zed Industries, Inc., pinned at 418f897.
fn classify_search_bytes(bytes: &[u8]) -> SearchByteClass {
    let prefix = &bytes[..bytes.len().min(SEARCH_PREFIX_BYTES)];
    if Encoding::for_bom(prefix).is_some() {
        SearchByteClass::Encoded
    } else if has_known_binary_header(prefix) {
        SearchByteClass::Binary
    } else if unmarked_utf16_encoding(prefix).is_some() {
        SearchByteClass::Encoded
    } else if bytes.contains(&0) || has_binary_control_bytes(bytes) {
        SearchByteClass::Binary
    } else {
        SearchByteClass::Utf8
    }
}

fn has_known_binary_header(bytes: &[u8]) -> bool {
    const HEADERS: &[&[u8]] = &[
        b"%PDF-",
        b"PK\x03\x04",
        b"PK\x05\x06",
        b"PK\x07\x08",
        b"\x89PNG\r\n\x1a\n",
        b"\xFF\xD8\xFF",
        b"GIF87a",
        b"GIF89a",
        b"IWAD",
        b"PWAD",
        b"RIFF",
        b"OggS",
        b"fLaC",
        b"ID3",
        b"\xFF\xFB",
        b"\xFF\xFA",
        b"\xFF\xF3",
        b"\xFF\xF2",
        b"\x1F\x8B",
        b"\xFD7zXZ\x00",
        b"\x28\xB5\x2F\xFD",
        b"\x04\x22\x4D\x18",
        b"7z\xBC\xAF\x27\x1C",
        b"Rar!\x1A\x07",
    ];
    HEADERS.iter().any(|header| bytes.starts_with(header))
        || (bytes.starts_with(b"BZh")
            && bytes
                .get(3)
                .is_some_and(|block_size| (b'1'..=b'9').contains(block_size)))
}

fn unmarked_utf16_encoding(bytes: &[u8]) -> Option<&'static Encoding> {
    let sample_length = bytes.len().min(SEARCH_PREFIX_BYTES) & !1;
    if sample_length < 4 {
        return None;
    }
    let sample = &bytes[..sample_length];
    let even_nuls = sample.iter().step_by(2).filter(|byte| **byte == 0).count();
    let odd_nuls = sample
        .iter()
        .skip(1)
        .step_by(2)
        .filter(|byte| **byte == 0)
        .count();
    let total_nuls = even_nuls + odd_nuls;
    if total_nuls == 0 || total_nuls * 16 < sample_length {
        return None;
    }
    let encoding = if even_nuls > odd_nuls.saturating_mul(4) {
        encoding_rs::UTF_16BE
    } else if odd_nuls > even_nuls.saturating_mul(4) {
        encoding_rs::UTF_16LE
    } else {
        return None;
    };
    is_plausible_utf16_text(sample, encoding == encoding_rs::UTF_16LE)
        .then_some(encoding)
}

fn is_plausible_utf16_text(bytes: &[u8], little_endian: bool) -> bool {
    let code_unit_at = |offset: usize| {
        let pair = [*bytes.get(offset)?, *bytes.get(offset + 1)?];
        Some(if little_endian {
            u16::from_le_bytes(pair)
        } else {
            u16::from_be_bytes(pair)
        })
    };
    let mut suspicious_count = 0usize;
    let mut word_like_count = 0usize;
    let mut total = 0usize;
    let mut offset = 0;
    while let Some(code_unit) = code_unit_at(offset) {
        total += 1;
        match code_unit {
            0x0009 | 0x000A | 0x000C | 0x000D => {}
            0x0020 | 0x0030..=0x0039 | 0x0041..=0x005A | 0x0061..=0x007A => {
                word_like_count += 1;
            }
            0x0000..=0x001F | 0x007F..=0x009F | 0xFFFE | 0xFFFF => {
                suspicious_count += 1;
            }
            0xD800..=0xDBFF => {
                if code_unit_at(offset + 2)
                    .is_some_and(|next| (0xDC00..=0xDFFF).contains(&next))
                {
                    total += 1;
                    word_like_count += 2;
                    offset += 2;
                } else {
                    suspicious_count += 1;
                }
            }
            0xDC00..=0xDFFF => suspicious_count += 1,
            0x0100.. => word_like_count += 1,
            _ => {}
        }
        offset += 2;
    }
    total > 0
        && suspicious_count * 100 < total * 2
        && word_like_count * 100 >= total * 30
}

fn has_binary_control_bytes(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }
    let control_bytes = bytes
        .iter()
        .filter(|byte| {
            matches!(**byte, 0x00..=0x08 | 0x0b | 0x0e..=0x1a | 0x1c..=0x1f | 0x7f)
        })
        .count();
    control_bytes * 10 > bytes.len()
}

fn read_search_file<F>(
    file: &mut File,
    path: &Path,
    should_continue: &F,
) -> Result<Option<Vec<u8>>, FileSearchError>
where
    F: Fn() -> bool,
{
    match file.rewind() {
        Ok(()) => {}
        Err(error) => {
            tracing::debug!(path = %path.display(), %error, "skipping unreadable search path");
            return Ok(None);
        }
    };
    let mut contents = Vec::new();
    let mut chunk = [0; 64 * 1024];
    loop {
        if !should_continue() {
            return Err(FileSearchError::Cancelled);
        }
        match file.read(&mut chunk) {
            Ok(0) => return Ok(Some(contents)),
            Ok(bytes_read) => contents.extend_from_slice(&chunk[..bytes_read]),
            Err(error) => {
                tracing::debug!(
                    path = %path.display(),
                    %error,
                    "skipping unreadable search path"
                );
                return Ok(None);
            }
        }
    }
}

/// Disk reads stay rooted to the selected worktree. BuffersOnly cannot open
/// files, even if a caller accidentally includes a disk path.
#[derive(Clone, Copy)]
pub enum SearchScope<'a> {
    Workspace(&'a Path),
    BuffersOnly,
}

struct SearchWorkspace<'a> {
    requested: &'a Path,
    canonical: PathBuf,
    #[cfg(unix)]
    directory: File,
}

impl<'a> SearchWorkspace<'a> {
    fn open(requested: &'a Path) -> io::Result<Self> {
        let canonical = requested.canonicalize()?;
        #[cfg(unix)]
        let directory = crate::secure_fs::open_canonical_directory(&canonical)?;
        Ok(Self {
            requested,
            canonical,
            #[cfg(unix)]
            directory,
        })
    }

    fn relative_path<'p>(&self, path: &'p Path) -> io::Result<&'p Path> {
        let relative = path
            .strip_prefix(self.requested)
            .or_else(|_| path.strip_prefix(&self.canonical))
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "search path is outside workspace",
                )
            })?;
        if relative.as_os_str().is_empty()
            || !relative
                .components()
                .all(|component| matches!(component, Component::Normal(_)))
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "search path is outside workspace",
            ));
        }
        Ok(relative)
    }

    fn open_file(&self, path: &Path) -> io::Result<File> {
        let relative = self.relative_path(path)?;
        self.open_relative_file(relative)
    }

    fn open_relative_file(&self, relative: &Path) -> io::Result<File> {
        #[cfg(unix)]
        {
            crate::secure_fs::open_relative_regular_file(&self.directory, relative)
        }
        #[cfg(not(unix))]
        {
            let canonical_file = self.canonical.join(relative).canonicalize()?;
            if !canonical_file.starts_with(&self.canonical) {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "search path is outside workspace",
                ));
            }
            File::open(canonical_file)
        }
    }
}

/// Follow Zed's `language::decode_text` boundary for BOM/legacy text while
/// keeping ordinary UTF-8 files on the streaming search path.
fn decode_search_text(bytes: &[u8]) -> Option<String> {
    if let Some((encoding, _)) = Encoding::for_bom(bytes) {
        let (text, _) = encoding.decode_with_bom_removal(bytes);
        return Some(text.into_owned());
    }
    if let Some(encoding) = unmarked_utf16_encoding(bytes) {
        let (text, _, has_errors) = encoding.decode(bytes);
        return (!has_errors).then(|| text.into_owned());
    }
    if bytes.contains(&0) || has_binary_control_bytes(bytes) {
        return None;
    }
    if let Ok(text) = std::str::from_utf8(bytes) {
        return Some(text.to_owned());
    }

    let mut detector = EncodingDetector::new();
    detector.feed(bytes, true);
    let encoding = detector.guess(None, true);
    let (text, _, _) = encoding.decode(bytes);
    Some(text.into_owned())
}

fn search_match_position(text: &str, offset: usize) -> (usize, usize) {
    let before = &text[..offset];
    let line = before.bytes().filter(|byte| *byte == b'\n').count();
    let line_start = before.rfind('\n').map_or(0, |index| index + 1);
    (line, offset - line_start)
}

fn search_match_preview(
    text: &str,
    match_start: usize,
    match_end: usize,
) -> (String, Range<usize>) {
    let line_start = text[..match_start].rfind('\n').map_or(0, |index| index + 1);
    let line_end = text[match_start..]
        .find('\n')
        .map_or(text.len(), |index| match_start + index);
    let raw_line = &text[line_start..line_end];
    let start_column = match_start - line_start;
    let line = if raw_line.ends_with('\r')
        && start_column < raw_line.len().saturating_sub(1)
    {
        &raw_line[..raw_line.len() - 1]
    } else {
        raw_line
    };
    let (preview_start, preview_end) = if line.len() <= 200 {
        (0, line.len())
    } else {
        let before = &line[..start_column];
        let preview_start = before
            .char_indices()
            .rev()
            .nth(79)
            .map(|(index, _)| index)
            .unwrap_or(0);
        let preview_end = line[preview_start..]
            .char_indices()
            .nth(200)
            .map(|(index, _)| preview_start + index)
            .unwrap_or(line.len());
        (preview_start, preview_end)
    };
    let preview = &line[preview_start..preview_end];
    let preview_match_start = start_column
        .saturating_sub(preview_start)
        .min(preview.len());
    let preview_match_end = match_end
        .saturating_sub(line_start)
        .min(line.len())
        .saturating_sub(preview_start)
        .min(preview.len())
        .max(preview_match_start);
    (preview.to_string(), preview_match_start..preview_match_end)
}

fn search_result_sink<'a, M, F>(
    matcher: &'a M,
    should_continue: &'a F,
    file_matches: &'a mut Vec<FileSearchMatch>,
    match_count: &'a mut usize,
    max_results: usize,
    cancelled: &'a mut bool,
    limit_reached: &'a mut bool,
) -> impl Sink<Error = io::Error> + 'a
where
    M: Matcher<Error = NoError> + 'a,
    F: Fn() -> bool + 'a,
{
    UTF8(move |line_number, line: &str| {
        if !should_continue() {
            *cancelled = true;
            return Ok(false);
        }
        matcher.find_iter(line.as_bytes(), |found| {
            if !should_continue() {
                *cancelled = true;
                return false;
            }
            if line.get(..found.start()).is_none()
                || line.get(found.end()..).is_none()
            {
                return true;
            }
            let (start_line_offset, start) =
                search_match_position(line, found.start());
            let (end_line_offset, end) = search_match_position(line, found.end());
            let first_line = line_number.saturating_sub(1) as usize;
            let (line_content, preview_match) =
                search_match_preview(line, found.start(), found.end());
            file_matches.push(FileSearchMatch {
                line: first_line + start_line_offset,
                start,
                end_line: first_line + end_line_offset,
                end,
                line_content,
                preview_match,
            });
            *match_count += 1;
            if *match_count >= max_results {
                *limit_reached = true;
                false
            } else {
                true
            }
        })?;
        Ok(!*cancelled && !*limit_reached)
    })
}

fn search_one_path<M>(
    path: &Path,
    relative_path: Option<&Path>,
    workspace: Option<&SearchWorkspace<'_>>,
    overrides: &HashMap<PathBuf, String>,
    matcher: &M,
    searcher: &mut Searcher,
    prefix_scratch: &mut Vec<u8>,
    options: &FileSearchOptions,
    should_continue: &(impl Fn() -> bool + Sync),
) -> Result<Vec<FileSearchMatch>, FileSearchError>
where
    M: Matcher<Error = NoError>,
{
    if !overrides.contains_key(path) && workspace.is_none() {
        return Ok(Vec::new());
    }
    let decoded_bytes = overrides.get(path).map(String::as_bytes);
    let mut file_matches = Vec::new();
    let mut match_count = 0;
    let mut cancelled = false;
    let mut limit_reached = false;
    let mut disk_file = None;

    let initial_result = if let Some(bytes) = decoded_bytes {
        searcher.search_slice(
            matcher,
            bytes,
            search_result_sink(
                matcher,
                should_continue,
                &mut file_matches,
                &mut match_count,
                options.max_results,
                &mut cancelled,
                &mut limit_reached,
            ),
        )
    } else {
        let Some(workspace) = workspace else {
            return Ok(Vec::new());
        };
        let Some(relative_path) = relative_path else {
            return Ok(Vec::new());
        };
        let mut file = match workspace.open_relative_file(relative_path) {
            Ok(file) => file,
            Err(error) => {
                tracing::debug!(path = %path.display(), %error, "skipping unreadable search path");
                return Ok(Vec::new());
            }
        };
        let result = searcher.search_reader(
            matcher,
            Utf8ValidatingReader::new(&mut file, prefix_scratch),
            search_result_sink(
                matcher,
                should_continue,
                &mut file_matches,
                &mut match_count,
                options.max_results,
                &mut cancelled,
                &mut limit_reached,
            ),
        );
        disk_file = Some(file);
        result
    };

    let result = match initial_result {
        Err(error)
            if error.kind() == io::ErrorKind::InvalidData
                && overrides.get(path).is_none() =>
        {
            let Some(file) = disk_file.as_mut() else {
                return Ok(Vec::new());
            };
            match read_search_file(file, path, should_continue)?
                .and_then(|bytes| decode_search_text(&bytes))
            {
                Some(decoded) => {
                    file_matches.clear();
                    match_count = 0;
                    cancelled = false;
                    limit_reached = false;
                    searcher.search_slice(
                        matcher,
                        decoded.as_bytes(),
                        search_result_sink(
                            matcher,
                            should_continue,
                            &mut file_matches,
                            &mut match_count,
                            options.max_results,
                            &mut cancelled,
                            &mut limit_reached,
                        ),
                    )
                }
                None => return Ok(Vec::new()),
            }
        }
        result => result,
    };

    if cancelled || !should_continue() {
        return Err(FileSearchError::Cancelled);
    }
    if let Err(error) = result {
        tracing::debug!(path = %path.display(), %error, "skipping unreadable search path");
        return Ok(Vec::new());
    }
    Ok(file_matches)
}

/// Look-around can span lines, so regex searches decode each file before
/// matching; literal searches keep the streaming grep path.
fn search_regex_one_path<F>(
    path: &Path,
    relative_path: Option<&Path>,
    workspace: Option<&SearchWorkspace<'_>>,
    overrides: &HashMap<PathBuf, String>,
    matcher: &FancyRegex,
    options: &FileSearchOptions,
    should_continue: &F,
) -> Result<Vec<FileSearchMatch>, FileSearchError>
where
    F: Fn() -> bool,
{
    if !should_continue() {
        return Err(FileSearchError::Cancelled);
    }
    let text = if let Some(text) = overrides.get(path) {
        if is_binary_content(text.as_bytes()) {
            return Ok(Vec::new());
        }
        Cow::Borrowed(text.as_str())
    } else {
        let Some(workspace) = workspace else {
            return Ok(Vec::new());
        };
        let Some(relative_path) = relative_path else {
            return Ok(Vec::new());
        };
        let mut file = match workspace.open_relative_file(relative_path) {
            Ok(file) => file,
            Err(error) => {
                tracing::debug!(path = %path.display(), %error, "skipping unreadable search path");
                return Ok(Vec::new());
            }
        };
        let Some(bytes) = read_search_file(&mut file, path, should_continue)? else {
            return Ok(Vec::new());
        };
        if is_binary_content(&bytes) {
            return Ok(Vec::new());
        }
        let Some(text) = decode_search_text(&bytes) else {
            return Ok(Vec::new());
        };
        Cow::Owned(text)
    };

    let mut file_matches = Vec::new();
    for result in matcher.find_iter(text.as_ref()).take(options.max_results) {
        if !should_continue() {
            return Err(FileSearchError::Cancelled);
        }
        let found = result
            .map_err(|error| FileSearchError::RegexExecution(error.to_string()))?;
        let (start_line_offset, start) =
            search_match_position(text.as_ref(), found.start());
        let (end_line_offset, end) =
            search_match_position(text.as_ref(), found.end());
        let (line_content, preview_match) =
            search_match_preview(text.as_ref(), found.start(), found.end());
        file_matches.push(FileSearchMatch {
            line: start_line_offset,
            start,
            end_line: end_line_offset,
            end,
            line_content,
            preview_match,
        });
    }
    if !should_continue() {
        return Err(FileSearchError::Cancelled);
    }
    Ok(file_matches)
}

fn whole_word_pattern(pattern: String, whole_word: bool) -> String {
    if whole_word {
        format!(r"(?<!\w)(?:{pattern})(?!\w)")
    } else {
        pattern
    }
}

/// Restrict content search to paths within one worktree. Like Zed's project
/// search, a pattern may name a worktree-relative path or include its root
/// directory name. An exclusion wins over an inclusion.
pub struct WorkspacePathFilter {
    workspace: PathBuf,
    include: Option<PathPatternSet>,
    exclude: Option<PathPatternSet>,
}

struct PathPatternSet {
    globs: GlobSet,
    literal_paths: Vec<PathBuf>,
}

impl PathPatternSet {
    fn new(
        kind: &str,
        input: Option<&str>,
    ) -> Result<Option<Self>, FileSearchError> {
        let Some(input) = input else {
            return Ok(None);
        };
        let patterns = split_path_patterns(input);
        if patterns.is_empty() {
            return Ok(None);
        }
        let mut builder = GlobSetBuilder::new();
        let mut literal_paths = Vec::new();
        for pattern in patterns {
            let glob = Glob::new(pattern).map_err(|error| {
                FileSearchError::InvalidGlob(format!("{kind} glob: {error}"))
            })?;
            if !pattern.chars().any(|character| {
                matches!(character, '*' | '?' | '[' | ']' | '{' | '}' | '\\')
            }) {
                literal_paths.push(PathBuf::from(pattern));
            }
            builder.add(glob);
        }
        let globs = builder.build().map_err(|error| {
            FileSearchError::InvalidGlob(format!("{kind} glob: {error}"))
        })?;
        Ok(Some(Self {
            globs,
            literal_paths,
        }))
    }

    fn matches(&self, path: &Path) -> bool {
        self.globs.is_match(path)
            || self
                .literal_paths
                .iter()
                .any(|literal| path.starts_with(literal) || path.ends_with(literal))
    }
}

fn split_path_patterns(input: &str) -> Vec<&str> {
    let mut patterns = Vec::new();
    let mut start = 0;
    let mut brace_depth = 0usize;
    let mut in_class = false;
    let mut class_has_content = false;
    let mut escaped = false;
    for (index, character) in input.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' => escaped = true,
            '[' if !in_class => {
                in_class = true;
                class_has_content = false;
            }
            ']' if in_class && !class_has_content => {
                class_has_content = true;
            }
            ']' if in_class => in_class = false,
            '!' | '^' if in_class && !class_has_content => {}
            '{' if !in_class => brace_depth += 1,
            '}' if !in_class => brace_depth = brace_depth.saturating_sub(1),
            ',' if !in_class && brace_depth == 0 => {
                let pattern = input[start..index].trim();
                if !pattern.is_empty() {
                    patterns.push(pattern);
                }
                start = index + character.len_utf8();
            }
            _ if in_class => class_has_content = true,
            _ => {}
        }
    }
    let pattern = input[start..].trim();
    if !pattern.is_empty() {
        patterns.push(pattern);
    }
    patterns
}

impl WorkspacePathFilter {
    pub fn new(
        workspace: &Path,
        include: Option<&str>,
        exclude: Option<&str>,
    ) -> Result<Self, FileSearchError> {
        Ok(Self {
            workspace: workspace.to_path_buf(),
            include: PathPatternSet::new("include", include)?,
            exclude: PathPatternSet::new("exclude", exclude)?,
        })
    }

    pub fn matches(&self, path: &Path) -> bool {
        let Ok(relative) = path.strip_prefix(&self.workspace) else {
            return false;
        };
        let named_path = self
            .workspace
            .file_name()
            .map(|name| PathBuf::from(name).join(relative));
        let matches = |matcher: &PathPatternSet| {
            matcher.matches(relative)
                || named_path
                    .as_ref()
                    .is_some_and(|named_path| matcher.matches(named_path))
        };
        self.include.as_ref().is_none_or(&matches)
            && self
                .exclude
                .as_ref()
                .is_none_or(|matcher| !matches(matcher))
    }
}

/// Returns the visible files in a worktree using the same ignore semantics as
/// the editor's existing project search. Hidden and ignored paths stay out of
/// the model context unless the editor explicitly adds them later.
pub fn workspace_files(workspace: &Path) -> Vec<PathBuf> {
    workspace_paths(workspace).collect()
}

/// Iterate files lazily so a superseded search can stop without first
/// walking the entire workspace.
pub fn workspace_paths(workspace: &Path) -> impl Iterator<Item = PathBuf> + '_ {
    workspace_entries(workspace)
        .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
        .map(|entry| entry.into_path())
}

/// Fuzzy-rank one worktree snapshot for Quick Open. This follows Zed's
/// `fuzzy_nucleo::match_path_sets` scoring while leaving indexing and opening
/// with the editor. Candidate paths must be absolute paths under `workspace`;
/// paths outside it are ignored. `relative_to`, when present, is a
/// worktree-relative directory used to prefer nearby files on score ties.
pub fn rank_file_paths<I, F>(
    workspace: &Path,
    paths: I,
    query: &str,
    relative_to: Option<&Path>,
    max_results: usize,
    mut should_continue: F,
) -> Result<Vec<RankedPathMatch>, FileSearchError>
where
    I: IntoIterator<Item = PathBuf>,
    F: FnMut() -> bool,
{
    let query = query.split_whitespace().collect::<Vec<_>>().join(" ");
    if query.is_empty() || max_results == 0 {
        return Ok(Vec::new());
    }
    let query_chars = query.chars().any(char::is_uppercase).then(|| {
        query
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect::<Vec<_>>()
    });

    let pattern = Pattern::new(
        &query,
        CaseMatching::Ignore,
        Normalization::Smart,
        AtomKind::Fuzzy,
    );
    let mut config = Config::DEFAULT;
    config.set_match_paths();
    let mut matcher = FuzzyMatcher::new(config);
    let mut results = Vec::new();
    let result_buffer_limit = max_results.saturating_mul(4).max(128);
    let mut candidate_chars = Vec::new();
    let mut matched_chars = Vec::new();
    let mut path_chars = Vec::new();

    for path in paths {
        if !should_continue() {
            return Err(FileSearchError::Cancelled);
        }
        let Ok(relative) = path.strip_prefix(workspace) else {
            continue;
        };
        let relative_path = relative
            .components()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        matched_chars.clear();
        let haystack = Utf32Str::new(&relative_path, &mut candidate_chars);
        let Some(raw_score) =
            pattern.indices(haystack, &mut matcher, &mut matched_chars)
        else {
            continue;
        };

        let case_mismatches = count_case_mismatches(
            query_chars.as_deref(),
            &matched_chars,
            &relative_path,
            &mut path_chars,
        );
        matched_chars.sort_unstable();
        matched_chars.dedup();
        let filename_bonus =
            path_filename_match_bonus(&relative_path, &pattern, &mut matcher);
        let score = (raw_score as f64 + filename_bonus)
            * case_penalty(case_mismatches)
            - relative_path.len() as f64 * 0.01;
        let distance_to_relative_directory = relative_to
            .map_or(usize::MAX, |directory| path_distance(relative, directory));
        results.push(RankedPathMatch {
            path,
            relative_path,
            score,
            positions: matched_chars
                .iter()
                .map(|position| *position as usize)
                .collect(),
            distance_to_relative_directory,
        });
        if results.len() >= result_buffer_limit {
            sort_ranked_path_matches(&mut results);
            results.truncate(max_results);
        }
    }

    sort_ranked_path_matches(&mut results);
    results.truncate(max_results);
    Ok(results)
}

fn sort_ranked_path_matches(results: &mut [RankedPathMatch]) {
    results.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| {
                left.distance_to_relative_directory
                    .cmp(&right.distance_to_relative_directory)
            })
            .then_with(|| left.relative_path.cmp(&right.relative_path))
    });
}

fn count_case_mismatches(
    query_chars: Option<&[char]>,
    matched_chars: &[u32],
    candidate: &str,
    candidate_chars: &mut Vec<char>,
) -> u32 {
    let Some(query_chars) = query_chars else {
        return 0;
    };
    if query_chars.len() != matched_chars.len() {
        return 0;
    }
    candidate_chars.clear();
    candidate_chars.extend(candidate.chars());
    query_chars
        .iter()
        .zip(matched_chars)
        .filter(|(query_character, position)| {
            candidate_chars.get(**position as usize).is_some_and(
                |candidate_character| {
                    candidate_character != *query_character
                        && candidate_character.eq_ignore_ascii_case(query_character)
                },
            )
        })
        .count() as u32
}

fn case_penalty(mismatches: u32) -> f64 {
    if mismatches == 0 {
        1.0
    } else {
        0.9_f64.powi(mismatches as i32)
    }
}

fn path_filename_match_bonus(
    path: &str,
    pattern: &Pattern,
    matcher: &mut FuzzyMatcher,
) -> f64 {
    let Some(filename) = Path::new(path)
        .file_name()
        .map(|filename| filename.to_string_lossy())
        .filter(|filename| !filename.is_empty())
    else {
        return 0.0;
    };
    let mut characters = Vec::new();
    let haystack = Utf32Str::new(&filename, &mut characters);
    let score = pattern
        .atoms
        .iter()
        .filter_map(|atom| atom.score(haystack, matcher))
        .map(u32::from)
        .sum::<u32>();
    score as f64 / filename.len().max(1) as f64
}

fn path_distance(path: &Path, relative_to: &Path) -> usize {
    let mut path_components = path.components().peekable();
    let mut relative_components = relative_to.components().peekable();
    loop {
        match (path_components.peek(), relative_components.peek()) {
            (Some(path_component), Some(relative_component))
                if path_component == relative_component =>
            {
                path_components.next();
                relative_components.next();
            }
            _ => break,
        }
    }
    path_components.count() + relative_components.count() + 1
}

fn workspace_entries(
    workspace: &Path,
) -> impl Iterator<Item = ignore::DirEntry> + '_ {
    let mut walker = ignore::WalkBuilder::new(workspace);
    walker.require_git(false);
    walker.sort_by_file_name(|left, right| left.cmp(right));
    walker.build().filter_map(Result::ok)
}

#[derive(Default)]
struct WorkspaceIndexWalkOutput {
    files: Vec<PathBuf>,
    visible_entries: HashSet<PathBuf>,
}

struct WorkspaceIndexWalkCollector {
    files: Vec<PathBuf>,
    visible_entries: HashSet<PathBuf>,
    output: Arc<Mutex<WorkspaceIndexWalkOutput>>,
}

impl Drop for WorkspaceIndexWalkCollector {
    fn drop(&mut self) {
        let mut output = self
            .output
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        output.files.append(&mut self.files);
        output.visible_entries.extend(self.visible_entries.drain());
    }
}

fn workspace_index_entries<F>(
    workspace: &Path,
    should_continue: Arc<F>,
) -> Option<WorkspaceIndexWalkOutput>
where
    F: Fn() -> bool + Send + Sync + 'static,
{
    let cancelled = Arc::new(AtomicBool::new(false));
    let output = Arc::new(Mutex::new(WorkspaceIndexWalkOutput::default()));
    let mut walker = ignore::WalkBuilder::new(workspace);
    walker.require_git(false);
    walker.build_parallel().run({
        let should_continue = should_continue.clone();
        let cancelled = cancelled.clone();
        let output = output.clone();
        move || {
            let mut collector = WorkspaceIndexWalkCollector {
                files: Vec::new(),
                visible_entries: HashSet::new(),
                output: output.clone(),
            };
            let should_continue = should_continue.clone();
            let cancelled = cancelled.clone();
            Box::new(move |entry| {
                if cancelled.load(Ordering::Acquire) {
                    return ignore::WalkState::Quit;
                }
                if !should_continue() {
                    cancelled.store(true, Ordering::Release);
                    return ignore::WalkState::Quit;
                }
                let Ok(entry) = entry else {
                    return ignore::WalkState::Continue;
                };
                let Some(kind) = entry.file_type() else {
                    return ignore::WalkState::Continue;
                };
                let path = entry.into_path();
                if kind.is_file() {
                    collector.files.push(path.clone());
                }
                if kind.is_file() || kind.is_dir() {
                    collector.visible_entries.insert(path);
                }
                ignore::WalkState::Continue
            })
        }
    });
    if cancelled.load(Ordering::Acquire) {
        return None;
    }
    let mut output = output.lock().unwrap_or_else(|error| error.into_inner());
    output.files.sort_unstable();
    Some(WorkspaceIndexWalkOutput {
        files: std::mem::take(&mut output.files),
        visible_entries: std::mem::take(&mut output.visible_entries),
    })
}

/// A worktree path snapshot. The proxy invalidates it on file-set and ignore
/// rule changes; searches use one stable path view while reading live content.
#[derive(Debug)]
pub struct WorkspaceFileIndex {
    workspace: PathBuf,
    watched_workspace: PathBuf,
    state: Mutex<WorkspaceFileIndexState>,
    ready: Condvar,
}

static WORKSPACE_SEARCH_EPOCH: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Default)]
struct WorkspaceFileIndexState {
    generation: u64,
    search_revision: u64,
    building: bool,
    files: Option<Arc<Vec<PathBuf>>>,
    visible_entries: Option<Arc<HashSet<PathBuf>>>,
}

impl WorkspaceFileIndex {
    pub fn new(workspace: PathBuf) -> Self {
        let watched_workspace = workspace
            .canonicalize()
            .unwrap_or_else(|_| workspace.clone());
        // Prevent persisted cursors from being reused with a newly created index.
        let search_revision = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64
            ^ u64::from(std::process::id()).rotate_left(32)
            ^ WORKSPACE_SEARCH_EPOCH.fetch_add(1, Ordering::Relaxed);
        Self {
            workspace,
            watched_workspace,
            state: Mutex::new(WorkspaceFileIndexState {
                search_revision,
                ..WorkspaceFileIndexState::default()
            }),
            ready: Condvar::new(),
        }
    }

    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    pub fn snapshot(&self) -> Arc<Vec<PathBuf>> {
        self.snapshot_with_generation().1
    }

    pub fn snapshot_with_generation(&self) -> (u64, Arc<Vec<PathBuf>>) {
        loop {
            if let Some(snapshot) = self.snapshot_with_generation_while(|| true) {
                return snapshot;
            }
        }
    }

    /// Stop a cold index walk when its caller is superseded. A cancelled
    /// builder releases waiting searches so one of them can finish the walk.
    pub fn snapshot_with_generation_while(
        &self,
        should_continue: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Option<(u64, Arc<Vec<PathBuf>>)> {
        self.snapshot_with_search_revision_while(should_continue)
            .map(|(generation, _, files)| (generation, files))
    }

    /// Return the indexed paths and a revision that changes for relevant
    /// content and file-set updates, without rebuilding the path list for
    /// content-only edits.
    pub fn snapshot_with_search_revision_while(
        &self,
        should_continue: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Option<(u64, u64, Arc<Vec<PathBuf>>)> {
        let should_continue = Arc::new(should_continue);
        loop {
            if !should_continue() {
                return None;
            }
            let generation = {
                let mut state =
                    self.state.lock().unwrap_or_else(|error| error.into_inner());
                while state.building {
                    let (next_state, _) = self
                        .ready
                        .wait_timeout(state, std::time::Duration::from_millis(100))
                        .unwrap_or_else(|error| error.into_inner());
                    state = next_state;
                    if !should_continue() {
                        return None;
                    }
                }
                if let Some(files) = &state.files {
                    return Some((
                        state.generation,
                        state.search_revision,
                        files.clone(),
                    ));
                }
                state.building = true;
                state.generation
            };
            let Some(WorkspaceIndexWalkOutput {
                files,
                visible_entries,
            }) = workspace_index_entries(&self.workspace, should_continue.clone())
            else {
                let mut state =
                    self.state.lock().unwrap_or_else(|error| error.into_inner());
                state.building = false;
                self.ready.notify_all();
                return None;
            };
            let files = Arc::new(files);
            let mut state =
                self.state.lock().unwrap_or_else(|error| error.into_inner());
            state.building = false;
            self.ready.notify_all();
            if !should_continue() {
                return None;
            }
            if state.generation == generation {
                if state.files.is_none() {
                    state.visible_entries = Some(Arc::new(visible_entries));
                }
                let files = state.files.get_or_insert(files).clone();
                return Some((generation, state.search_revision, files));
            }
        }
    }

    /// A cached worktree entry or a new child of a visible directory can
    /// change search results. Unknown state is conservatively relevant.
    pub fn affects_search(&self, path: &Path, file_set_change: bool) -> bool {
        let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let Some(entries) = &state.visible_entries else {
            return true;
        };
        let relative = path
            .strip_prefix(&self.workspace)
            .or_else(|_| path.strip_prefix(&self.watched_workspace));
        let Ok(relative) = relative else {
            return true;
        };
        let path = self.workspace.join(relative);
        entries.contains(&path)
            || (file_set_change
                && path.parent().is_some_and(|parent| entries.contains(parent)))
    }

    pub fn generation(&self) -> u64 {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .generation
    }

    pub fn search_revision(&self) -> u64 {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .search_revision
    }

    pub fn mark_content_changed(&self) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.search_revision = state.search_revision.wrapping_add(1);
    }

    pub fn invalidate(&self) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        // In-flight walks still need a generation fence; an idle empty cache does not.
        if state.files.is_none() && !state.building {
            return;
        }
        state.generation = state.generation.wrapping_add(1);
        state.search_revision = state.search_revision.wrapping_add(1);
        state.files = None;
        state.visible_entries = None;
    }
}

/// Agent-visible workspace files. Like Zed's grep tool, keep credentials out
/// of model search even when a project has not gitignored them.
pub fn agent_workspace_paths(
    workspace: &Path,
) -> impl Iterator<Item = PathBuf> + '_ {
    workspace_paths(workspace)
        .filter(move |path| is_agent_visible_path(workspace, path))
}

pub fn is_agent_visible_path(workspace: &Path, path: &Path) -> bool {
    path.strip_prefix(workspace)
        .is_ok_and(|relative| !is_private_file(relative))
}

/// Apply default private-file patterns to a worktree-relative path.
pub fn is_private_file(path: &Path) -> bool {
    // Open buffers bypass the filesystem walk's hidden-directory filtering.
    // The workspace-private AHEAD directory can contain model credentials.
    if path
        .components()
        .any(|component| component.as_os_str() == ".ahead")
    {
        return true;
    }
    path.components().any(|component| {
        let Some(name) = component.as_os_str().to_str() else {
            return false;
        };
        name.starts_with(".env")
            || name == "secrets.yml"
            || name.ends_with(".pem")
            || name.ends_with(".key")
            || name.ends_with(".cert")
            || name.ends_with(".crt")
    })
}

/// Resolve a workspace-relative open buffer without following a symlink out
/// of the project. Unsaved files are allowed when their parent already exists.
pub fn resolve_open_buffer_path(
    workspace: &Path,
    relative: &Path,
) -> Option<PathBuf> {
    if relative.as_os_str().is_empty()
        || !relative
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        return None;
    }
    let root = workspace.canonicalize().ok()?;
    let mut path = workspace.to_path_buf();
    for component in relative.components() {
        path.push(component.as_os_str());
        if path.is_symlink() {
            return None;
        }
    }
    if path.exists() && !path.is_file() {
        return None;
    }
    let resolved = if path.exists() {
        path.canonicalize().ok()?
    } else {
        path.parent()?.canonicalize().ok()?.join(path.file_name()?)
    };
    resolved.starts_with(root).then_some(path)
}

/// Return unsaved new editor files before the worktree walk. Existing ignored
/// files stay subject to the worktree's ignore rules, as in Zed's project
/// search; new entryless buffers remain searchable.
pub fn new_open_buffer_paths(
    workspace: &Path,
    overrides: &HashMap<PathBuf, String>,
) -> Vec<PathBuf> {
    let mut paths = overrides
        .keys()
        .filter(|path| !path.exists())
        .filter(|path| {
            path.strip_prefix(workspace)
                .ok()
                .and_then(|relative| resolve_open_buffer_path(workspace, relative))
                .is_some_and(|resolved| resolved.as_path() == path.as_path())
        })
        .cloned()
        .collect::<Vec<_>>();
    paths.sort_unstable();
    paths
}

/// Search paths with editor-compatible matcher and cancellation behavior.
/// Search I/O failures are skipped like Zed's project search skips files that
/// disappear during a scan; a malformed query and cancellation are surfaced.
pub fn search_paths<I, F>(
    scope: SearchScope<'_>,
    paths: I,
    options: &FileSearchOptions,
    should_continue: F,
) -> Result<Vec<(PathBuf, Vec<FileSearchMatch>)>, FileSearchError>
where
    I: IntoIterator<Item = PathBuf>,
    F: Fn() -> bool + Sync,
{
    search_paths_with_overrides(
        scope,
        paths,
        &HashMap::new(),
        options,
        should_continue,
    )
}

/// Replace every match in one editor buffer using the same text, whole-word,
/// regex, and case options as project search. Text-mode replacements are
/// literal; regex replacements expand capture groups and Zed-style `\\n`,
/// `\\t`, and `\\\\` escapes.
pub fn replace_text_matches(
    text: &str,
    options: &FileSearchOptions,
    replacement: &str,
) -> Result<(String, usize), FileSearchError> {
    if options.pattern.is_empty() {
        return Ok((text.to_owned(), 0));
    }
    let matcher = replacement_matcher(options)?;
    replace_text_with_matcher(
        text,
        options,
        replacement,
        &matcher,
        MAX_SEARCH_RESULT_RANGES,
    )
}

/// Replace one previously searched match in a workspace file without writing
/// it to disk. Dirty editor contents take precedence over disk.
pub fn replace_path_match_with_overrides<F>(
    scope: SearchScope<'_>,
    path: &Path,
    overrides: &HashMap<PathBuf, String>,
    options: &FileSearchOptions,
    replacement: &str,
    matched: &FileSearchMatch,
    should_continue: F,
) -> Result<Option<FileSearchReplacement>, FileSearchError>
where
    F: Fn() -> bool,
{
    if options.pattern.is_empty() {
        return Ok(None);
    }
    if !should_continue() {
        return Err(FileSearchError::Cancelled);
    }

    let workspace = match scope {
        SearchScope::Workspace(path) => {
            Some(SearchWorkspace::open(path).map_err(|error| {
                FileSearchError::InvalidWorkspace(error.to_string())
            })?)
        }
        SearchScope::BuffersOnly => None,
    };
    if let Some(workspace) = workspace.as_ref() {
        workspace
            .relative_path(path)
            .map_err(|error| FileSearchError::InvalidWorkspace(error.to_string()))?;
    }

    let text = if let Some(text) = overrides.get(path) {
        text.clone()
    } else {
        let Some(workspace) = workspace.as_ref() else {
            return Ok(None);
        };
        let mut file = match workspace.open_file(path) {
            Ok(file) => file,
            Err(error) => {
                tracing::debug!(path = %path.display(), %error, "skipping unreadable search path");
                return Ok(None);
            }
        };
        let Some(bytes) = read_search_file(&mut file, path, &should_continue)?
        else {
            return Ok(None);
        };
        if is_binary_content(&bytes) {
            return Ok(None);
        }
        let Some(text) = decode_search_text(&bytes) else {
            return Ok(None);
        };
        text
    };

    let Some(replacement_text) =
        replace_text_match(&text, options, replacement, matched)?
    else {
        return Ok(None);
    };
    if !should_continue() {
        return Err(FileSearchError::Cancelled);
    }
    Ok(Some(FileSearchReplacement {
        path: path.to_path_buf(),
        original_text: text,
        replacement_text,
        match_count: 1,
    }))
}

fn replace_text_match(
    text: &str,
    options: &FileSearchOptions,
    replacement: &str,
    matched: &FileSearchMatch,
) -> Result<Option<String>, FileSearchError> {
    let Some(start) = search_offset_at_position(text, matched.line, matched.start)
    else {
        return Ok(None);
    };
    let Some(end) = search_offset_at_position(text, matched.end_line, matched.end)
    else {
        return Ok(None);
    };
    if start > end {
        return Ok(None);
    }

    let replacement = if options.is_regex {
        expand_regex_replacement_escapes(replacement)
    } else {
        replacement.to_owned()
    };
    let matcher = replacement_matcher(options)?;
    let mut replaced = false;
    let replacement_text = match matcher {
        ReplacementMatcher::Text(matcher) => matcher
            .replace_all(text, |captures: &regex::Captures<'_>| {
                let Some(found) = captures.get(0) else {
                    return String::new();
                };
                if !replaced && found.start() == start && found.end() == end {
                    replaced = true;
                    replacement.clone()
                } else {
                    found.as_str().to_owned()
                }
            })
            .into_owned(),
        ReplacementMatcher::Regex(matcher) => matcher
            .try_replacen(text, 0, |captures: &fancy_regex::Captures<'_, str>| {
                let Some(found) = captures.get(0) else {
                    return String::new();
                };
                if !replaced && found.start() == start && found.end() == end {
                    replaced = true;
                    if options.is_regex {
                        let mut expanded = String::new();
                        captures.expand(&replacement, &mut expanded);
                        expanded
                    } else {
                        replacement.clone()
                    }
                } else {
                    found.as_str().to_owned()
                }
            })
            .map_err(|error| FileSearchError::RegexExecution(error.to_string()))?
            .into_owned(),
    };
    Ok(replaced.then_some(replacement_text))
}

fn search_offset_at_position(
    text: &str,
    line: usize,
    column: usize,
) -> Option<usize> {
    let line_start = if line == 0 {
        0
    } else {
        text.match_indices('\n')
            .nth(line - 1)
            .map(|(offset, _)| offset + 1)?
    };
    let line_end = text[line_start..]
        .find('\n')
        .map_or(text.len(), |offset| line_start + offset);
    let line_text = text.get(line_start..line_end)?;
    if column > line_text.len() || !line_text.is_char_boundary(column) {
        return None;
    }
    Some(line_start + column)
}

enum ReplacementMatcher {
    Text(Regex),
    Regex(FancyRegex),
}

fn replacement_matcher(
    options: &FileSearchOptions,
) -> Result<ReplacementMatcher, FileSearchError> {
    let pattern = if options.is_regex {
        options.pattern.clone()
    } else {
        regex::escape(&options.pattern)
    };
    let pattern = whole_word_pattern(pattern, options.whole_word);
    if options.is_regex || options.whole_word {
        let mut builder = FancyRegexBuilder::new(&pattern);
        builder
            .case_insensitive(!options.case_sensitive)
            .multi_line(true)
            .crlf(true);
        builder
            .build()
            .map(ReplacementMatcher::Regex)
            .map_err(|error| FileSearchError::InvalidPattern(error.to_string()))
    } else {
        RegexBuilder::new(&pattern)
            .case_insensitive(!options.case_sensitive)
            .multi_line(true)
            .crlf(true)
            .build()
            .map(ReplacementMatcher::Text)
            .map_err(|error| FileSearchError::InvalidPattern(error.to_string()))
    }
}

fn replace_text_with_matcher(
    text: &str,
    options: &FileSearchOptions,
    replacement: &str,
    matcher: &ReplacementMatcher,
    remaining_matches: usize,
) -> Result<(String, usize), FileSearchError> {
    let match_count = match matcher {
        ReplacementMatcher::Text(matcher) => matcher
            .find_iter(text)
            .take(remaining_matches.saturating_add(1))
            .count(),
        ReplacementMatcher::Regex(matcher) => {
            let mut count = 0;
            for result in matcher
                .find_iter(text)
                .take(remaining_matches.saturating_add(1))
            {
                result.map_err(|error| {
                    FileSearchError::RegexExecution(error.to_string())
                })?;
                count += 1;
            }
            count
        }
    };
    if match_count > remaining_matches {
        return Err(FileSearchError::TooManyMatches);
    }
    if match_count == 0 {
        return Ok((text.to_owned(), 0));
    }
    let replacement = if options.is_regex {
        expand_regex_replacement_escapes(replacement)
    } else {
        replacement.to_owned()
    };
    let replaced = match matcher {
        ReplacementMatcher::Text(matcher) => matcher
            .replace_all(text, NoExpand(replacement.as_str()))
            .into_owned(),
        ReplacementMatcher::Regex(matcher) if options.is_regex => matcher
            .try_replacen(text, 0, replacement.as_str())
            .map_err(|error| FileSearchError::RegexExecution(error.to_string()))?
            .into_owned(),
        ReplacementMatcher::Regex(matcher) => matcher
            .try_replacen(text, 0, FancyNoExpand(replacement.as_str()))
            .map_err(|error| FileSearchError::RegexExecution(error.to_string()))?
            .into_owned(),
    };
    Ok((replaced, match_count))
}

/// Precompute replacements for project files without writing to disk. Dirty
/// editor buffers in `overrides` take precedence over their saved contents.
/// Paths outside the worktree, unreadable files, and binary files are skipped
/// like project search; callers must validate editor snapshots before applying
/// any returned changes.
pub fn replace_paths_with_overrides<I, F>(
    scope: SearchScope<'_>,
    paths: I,
    overrides: &HashMap<PathBuf, String>,
    options: &FileSearchOptions,
    replacement: &str,
    should_continue: F,
) -> Result<Vec<FileSearchReplacement>, FileSearchError>
where
    I: IntoIterator<Item = PathBuf>,
    F: Fn() -> bool,
{
    if options.pattern.is_empty() {
        return Ok(Vec::new());
    }
    let matcher = replacement_matcher(options)?;
    let workspace = match scope {
        SearchScope::Workspace(path) => {
            Some(SearchWorkspace::open(path).map_err(|error| {
                FileSearchError::InvalidWorkspace(error.to_string())
            })?)
        }
        SearchScope::BuffersOnly => None,
    };
    let mut seen = HashSet::new();
    let mut replacements = Vec::new();
    let mut total_match_count = 0usize;
    for path in paths {
        if !should_continue() {
            return Err(FileSearchError::Cancelled);
        }
        if !seen.insert(path.clone()) {
            continue;
        }
        if let Some(workspace) = workspace.as_ref()
            && workspace.relative_path(&path).is_err()
        {
            continue;
        }
        let text = if let Some(text) = overrides.get(&path) {
            text.clone()
        } else {
            let Some(workspace) = workspace.as_ref() else {
                continue;
            };
            let Ok(mut file) = workspace.open_file(&path) else {
                continue;
            };
            let Some(bytes) = read_search_file(&mut file, &path, &should_continue)?
            else {
                continue;
            };
            let Some(text) = decode_search_text(&bytes) else {
                continue;
            };
            text
        };
        let remaining_matches =
            MAX_SEARCH_RESULT_RANGES.saturating_sub(total_match_count);
        let (replacement_text, match_count) = replace_text_with_matcher(
            &text,
            options,
            replacement,
            &matcher,
            remaining_matches,
        )?;
        total_match_count += match_count;
        if match_count > 0 && replacement_text != text {
            replacements.push(FileSearchReplacement {
                path,
                original_text: text,
                replacement_text,
                match_count,
            });
        }
    }
    Ok(replacements)
}

fn expand_regex_replacement_escapes(replacement: &str) -> String {
    let mut expanded = String::with_capacity(replacement.len());
    let mut characters = replacement.chars();
    while let Some(character) = characters.next() {
        if character != '\\' {
            expanded.push(character);
            continue;
        }
        match characters.next() {
            Some('\\') => expanded.push('\\'),
            Some('n') => expanded.push('\n'),
            Some('t') => expanded.push('\t'),
            Some(next) => {
                expanded.push('\\');
                expanded.push(next);
            }
            None => expanded.push('\\'),
        }
    }
    expanded
}

/// Search the current editor text for open paths, falling back to disk for
/// other paths. Callers provide only the buffers they already own; path
/// enumeration and ignore filtering remain at the project boundary.
pub fn search_paths_with_overrides<I, F>(
    scope: SearchScope<'_>,
    paths: I,
    overrides: &HashMap<PathBuf, String>,
    options: &FileSearchOptions,
    should_continue: F,
) -> Result<Vec<(PathBuf, Vec<FileSearchMatch>)>, FileSearchError>
where
    I: IntoIterator<Item = PathBuf>,
    F: Fn() -> bool + Sync,
{
    let mut matches = Vec::new();
    search_paths_with_overrides_stream(
        scope,
        paths,
        overrides,
        options,
        should_continue,
        |path, file_matches| {
            matches.push((path, file_matches));
            true
        },
    )?;
    Ok(matches)
}

/// Emit each matching file as soon as it finishes scanning. A bounded consumer
/// can apply backpressure while callers that need all results use the wrapper
/// above. The cancellation predicate is polled concurrently by search workers.
/// Returns whether the match limit was reached. Returning false from the
/// callback cancels the scan.
fn search_worker_count() -> usize {
    std::thread::available_parallelism()
        .map(|cores| cores.get().saturating_sub(1).max(1))
        .unwrap_or(1)
}

const SEARCH_PATHS_PER_JOB: usize = 8;

fn search_pool() -> Result<&'static ThreadPool, FileSearchError> {
    static SEARCH_POOL: OnceLock<Result<ThreadPool, String>> = OnceLock::new();
    match SEARCH_POOL.get_or_init(|| {
        ThreadPoolBuilder::new()
            .num_threads(search_worker_count())
            .thread_name(|index| format!("ahead-search-{index}"))
            .build()
            .map_err(|error| error.to_string())
    }) {
        Ok(pool) => Ok(pool),
        Err(error) => {
            tracing::error!(%error, "failed to initialize the shared search pool");
            Err(FileSearchError::WorkerFailed)
        }
    }
}

struct SearchWorkerState {
    searcher: Searcher,
    utf8_prefix: Vec<u8>,
}

thread_local! {
    static SEARCH_STATE: std::cell::RefCell<SearchWorkerState> = std::cell::RefCell::new(
        SearchWorkerState {
            searcher: SearcherBuilder::new()
                .line_terminator(LineTerminator::crlf())
                .multi_line(true)
                .binary_detection(BinaryDetection::quit(b'\0'))
                .build(),
            utf8_prefix: Vec::new(),
        }
    );
}

enum SearchMatcher {
    Literal(RegexMatcher),
    Regex(FancyRegex),
}

pub fn search_paths_with_overrides_stream<I, F, C>(
    scope: SearchScope<'_>,
    paths: I,
    overrides: &HashMap<PathBuf, String>,
    options: &FileSearchOptions,
    should_continue: F,
    mut on_file_matches: C,
) -> Result<bool, FileSearchError>
where
    I: IntoIterator<Item = PathBuf>,
    F: Fn() -> bool + Sync,
    C: FnMut(PathBuf, Vec<FileSearchMatch>) -> bool,
{
    if options.pattern.is_empty() || options.max_results == 0 {
        return Ok(false);
    }
    let workspace = match scope {
        SearchScope::Workspace(path) => {
            Some(SearchWorkspace::open(path).map_err(|error| {
                FileSearchError::InvalidWorkspace(error.to_string())
            })?)
        }
        SearchScope::BuffersOnly => None,
    };
    let matcher =
        if options.is_regex {
            let pattern =
                whole_word_pattern(options.pattern.clone(), options.whole_word);
            let mut matcher_builder = FancyRegexBuilder::new(&pattern);
            matcher_builder
                .case_insensitive(!options.case_sensitive)
                .multi_line(true)
                .crlf(true);
            SearchMatcher::Regex(matcher_builder.build().map_err(|error| {
                FileSearchError::InvalidPattern(error.to_string())
            })?)
        } else {
            let mut matcher_builder = RegexMatcherBuilder::new();
            matcher_builder
                .case_insensitive(!options.case_sensitive)
                .multi_line(true)
                .crlf(true)
                .word(options.whole_word);
            // Only single-line literals can use grep-regex's candidate-line prefilter.
            if options.pattern.contains('\r') || options.pattern.contains('\n') {
                matcher_builder.line_terminator(None);
            }
            SearchMatcher::Literal(
                matcher_builder
                    .build_literals(&[&options.pattern])
                    .map_err(|error| {
                        FileSearchError::InvalidPattern(error.to_string())
                    })?,
            )
        };
    let worker_count = search_worker_count();
    let pool = search_pool()?;
    pool.in_place_scope(|scope| {
        let (results_sender, results_receiver) = mpsc::channel::<(
            Vec<(
                usize,
                PathBuf,
                Result<Vec<FileSearchMatch>, FileSearchError>,
            )>,
            bool,
        )>();
        let mut results_sender = Some(results_sender);
        let keep_running = Arc::new(AtomicBool::new(true));
        let matcher = &matcher;
        let workspace = workspace.as_ref();
        let should_continue = &should_continue;

        let mut paths = paths.into_iter();
        let mut jobs_sent = 0usize;
        let mut next_result = 0usize;
        let mut in_flight = 0usize;
        let mut paths_exhausted = false;
        let mut match_count = 0usize;
        let mut pending = BTreeMap::new();
        let mut failure = None;
        let mut limit_reached = false;

        loop {
            while !paths_exhausted
                && in_flight < worker_count
                && jobs_sent.saturating_sub(next_result)
                    < worker_count.saturating_mul(SEARCH_PATHS_PER_JOB)
            {
                let mut batch = Vec::with_capacity(SEARCH_PATHS_PER_JOB);
                while batch.len() < SEARCH_PATHS_PER_JOB && !paths_exhausted {
                    if !should_continue() {
                        keep_running.store(false, Ordering::Relaxed);
                        failure = Some(FileSearchError::Cancelled);
                        break;
                    }
                    let Some(path) = paths.next() else {
                        paths_exhausted = true;
                        break;
                    };
                    let relative_path = match workspace {
                        Some(workspace) => match workspace.relative_path(&path) {
                            Ok(relative_path) => Some(relative_path.to_path_buf()),
                            Err(_) => continue,
                        },
                        None if overrides.contains_key(&path) => None,
                        None => continue,
                    };
                    batch.push((jobs_sent, path, relative_path));
                    jobs_sent += 1;
                }
                if failure.is_some() {
                    break;
                }
                if batch.is_empty() {
                    continue;
                }
                let Some(sender) = results_sender.as_ref() else {
                    keep_running.store(false, Ordering::Relaxed);
                    failure = Some(FileSearchError::WorkerFailed);
                    break;
                };
                let sender = sender.clone();
                let keep_running = keep_running.clone();
                // Batch misses to avoid task/channel overhead, but flush hits promptly.
                scope.spawn(move |_| {
                    let mut results = Vec::with_capacity(batch.len());
                    let batch_len = batch.len();
                    let mut batch_complete = false;
                    for (position, (job_index, path, relative_path)) in
                        batch.into_iter().enumerate()
                    {
                        if !keep_running.load(Ordering::Relaxed) {
                            break;
                        }
                        let worker_should_continue = || {
                            let continuing = keep_running.load(Ordering::Relaxed)
                                && should_continue();
                            if !continuing {
                                keep_running.store(false, Ordering::Relaxed);
                            }
                            continuing
                        };
                        let result = match matcher {
                            SearchMatcher::Literal(matcher) => {
                                SEARCH_STATE.with(|state| {
                                    let mut state = state.borrow_mut();
                                    let SearchWorkerState {
                                        searcher,
                                        utf8_prefix,
                                    } = &mut *state;
                                    search_one_path(
                                        &path,
                                        relative_path.as_deref(),
                                        workspace,
                                        overrides,
                                        matcher,
                                        searcher,
                                        utf8_prefix,
                                        options,
                                        &worker_should_continue,
                                    )
                                })
                            }
                            SearchMatcher::Regex(matcher) => search_regex_one_path(
                                &path,
                                relative_path.as_deref(),
                                workspace,
                                overrides,
                                matcher,
                                options,
                                &worker_should_continue,
                            ),
                        };
                        let flush = match &result {
                            Ok(matches) => !matches.is_empty(),
                            Err(_) => true,
                        };
                        results.push((job_index, path, result));
                        if flush {
                            batch_complete = position + 1 == batch_len;
                            let partial_results = std::mem::replace(
                                &mut results,
                                Vec::with_capacity(SEARCH_PATHS_PER_JOB),
                            );
                            if let Err(error) =
                                sender.send((partial_results, batch_complete))
                            {
                                tracing::debug!(
                                    result_count = error.0.0.len(),
                                    "search result receiver closed before worker batch completed"
                                );
                                return;
                            }
                        }
                    }
                    if !batch_complete {
                        if let Err(error) = sender.send((results, true)) {
                            tracing::debug!(
                                result_count = error.0.0.len(),
                                "search result receiver closed before worker batch completed"
                            );
                        }
                    }
                });
                in_flight += 1;
            }
            if paths_exhausted {
                results_sender = None;
            }
            if failure.is_some() || in_flight == 0 {
                break;
            }
            if !should_continue() {
                keep_running.store(false, Ordering::Relaxed);
                failure = Some(FileSearchError::Cancelled);
                break;
            }

            match results_receiver.recv_timeout(Duration::from_millis(10)) {
                Ok((results, batch_complete)) => {
                    if batch_complete {
                        in_flight -= 1;
                    }
                    for (job_index, path, result) in results {
                        pending.insert(job_index, (path, result));
                    }
                    while let Some((path, result)) = pending.remove(&next_result) {
                        if !should_continue() {
                            keep_running.store(false, Ordering::Relaxed);
                            failure = Some(FileSearchError::Cancelled);
                            break;
                        }
                        next_result += 1;
                        let mut file_matches = match result {
                            Ok(matches) => matches,
                            Err(error) => {
                                failure = Some(error);
                                keep_running.store(false, Ordering::Relaxed);
                                break;
                            }
                        };
                        file_matches.truncate(
                            options.max_results.saturating_sub(match_count),
                        );
                        match_count += file_matches.len();
                        if !file_matches.is_empty()
                            && !on_file_matches(path, file_matches)
                        {
                            failure = Some(FileSearchError::Cancelled);
                            keep_running.store(false, Ordering::Relaxed);
                            break;
                        }
                        if match_count >= options.max_results {
                            limit_reached = true;
                            keep_running.store(false, Ordering::Relaxed);
                            break;
                        }
                    }
                }
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => {
                    keep_running.store(false, Ordering::Relaxed);
                    failure = Some(FileSearchError::WorkerFailed);
                    break;
                }
            }
            if failure.is_some() || limit_reached {
                break;
            }
        }

        drop(results_sender);
        match failure {
            Some(error) => Err(error),
            None => Ok(match_count >= options.max_results),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Cursor;

    #[test]
    fn concurrent_searches_share_the_bounded_worker_pool() {
        let pool = search_pool().expect("search worker pool");
        assert_eq!(pool.current_num_threads(), search_worker_count());
        assert!(std::ptr::eq(
            pool,
            search_pool().expect("same search worker pool")
        ));
    }

    #[test]
    fn concurrent_workspace_searches_complete_from_shared_pool() {
        let workspace = tempfile::tempdir().expect("workspace");
        for index in 0..256 {
            fs::write(
                workspace.path().join(format!("file-{index:03}.rs")),
                "AHEAD_SHARED_POOL\n",
            )
            .expect("write search file");
        }
        let options = FileSearchOptions {
            pattern: "AHEAD_SHARED_POOL".into(),
            case_sensitive: true,
            whole_word: true,
            is_regex: false,
            max_results: 500,
        };

        std::thread::scope(|scope| {
            let searches = (0..8)
                .map(|_| {
                    let workspace = workspace.path();
                    let options = &options;
                    scope.spawn(move || {
                        search_paths(
                            SearchScope::Workspace(workspace),
                            workspace_files(workspace),
                            options,
                            || true,
                        )
                    })
                })
                .collect::<Vec<_>>();

            for search in searches {
                let results = search
                    .join()
                    .expect("search worker must finish")
                    .expect("independent search must not be cancelled");
                assert_eq!(results.len(), 256);
            }
        });
    }

    #[test]
    fn utf8_validator_handles_split_and_truncated_code_points() {
        let mut split_prefix = Vec::new();
        let mut split =
            Utf8ValidatingReader::new(Cursor::new([0xC3, 0xA9]), &mut split_prefix);
        let mut byte = [0; 1];
        assert_eq!(split.read(&mut byte).expect("first UTF-8 byte"), 1);
        assert_eq!(byte[0], 0xC3);
        assert_eq!(split.read(&mut byte).expect("second UTF-8 byte"), 1);
        assert_eq!(byte[0], 0xA9);
        assert_eq!(split.read(&mut byte).expect("end of UTF-8 stream"), 0);

        let mut truncated_prefix = Vec::new();
        let mut truncated =
            Utf8ValidatingReader::new(Cursor::new([0xC3]), &mut truncated_prefix);
        assert_eq!(truncated.read(&mut byte).expect("partial UTF-8 byte"), 1);
        assert_eq!(
            truncated
                .read(&mut byte)
                .expect_err("truncated UTF-8 must trigger decode fallback")
                .kind(),
            io::ErrorKind::InvalidData
        );

        let mut malformed_prefix = Vec::new();
        let mut malformed = Utf8ValidatingReader::new(
            Cursor::new([0xC3, 0x28]),
            &mut malformed_prefix,
        );
        let mut bytes = [0; 2];
        assert_eq!(
            malformed
                .read(&mut bytes)
                .expect_err("malformed UTF-8 must be rejected")
                .kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn utf8_validator_handles_code_point_split_at_classifier_boundary() {
        let mut expected = vec![b'x'; SEARCH_PREFIX_BYTES - 1];
        expected.extend_from_slice("é".as_bytes());
        let mut prefix = Vec::new();
        let mut reader =
            Utf8ValidatingReader::new(Cursor::new(expected.clone()), &mut prefix);
        let mut actual = Vec::new();

        reader
            .read_to_end(&mut actual)
            .expect("valid UTF-8 crossing the prefix boundary");

        assert_eq!(actual, expected);
    }

    #[test]
    fn utf8_validator_streams_large_multibyte_content() {
        let expected = "é🙂".repeat(32_768);
        let mut prefix = Vec::new();
        let mut reader =
            Utf8ValidatingReader::new(Cursor::new(expected.as_bytes()), &mut prefix);
        let mut actual = Vec::new();

        reader
            .read_to_end(&mut actual)
            .expect("large valid UTF-8 remains searchable");

        assert_eq!(actual, expected.as_bytes());
    }

    #[test]
    fn utf8_validator_reuses_prefix_capacity_between_files() {
        let mut prefix = Vec::new();
        let first = {
            let mut reader = Utf8ValidatingReader::new(
                Cursor::new(b"first short file".as_slice()),
                &mut prefix,
            );
            let mut contents = Vec::new();
            reader.read_to_end(&mut contents).expect("read first file");
            contents
        };
        let first_capacity = prefix.capacity();
        let first_pointer = prefix.as_ptr();
        let second = {
            let mut reader = Utf8ValidatingReader::new(
                Cursor::new(b"other short file".as_slice()),
                &mut prefix,
            );
            let mut contents = Vec::new();
            reader.read_to_end(&mut contents).expect("read second file");
            contents
        };

        assert_eq!(first, b"first short file");
        assert_eq!(second, b"other short file");
        assert_eq!(prefix.capacity(), first_capacity);
        assert_eq!(prefix.as_ptr(), first_pointer);
    }

    #[test]
    fn searches_literal_text_and_skips_gitignored_files() {
        let root = std::env::temp_dir()
            .join(format!("ahead-search-{}", std::process::id()));
        fs::create_dir_all(root.join("src")).expect("create source directory");
        fs::write(root.join("src/lib.rs"), "needle here\n").expect("write source");
        fs::write(root.join("ignored.txt"), "needle hidden\n")
            .expect("write ignored file");
        fs::write(root.join(".gitignore"), "ignored.txt\n")
            .expect("write ignore file");

        let options = FileSearchOptions {
            pattern: "needle".to_string(),
            case_sensitive: true,
            whole_word: false,
            is_regex: false,
            max_results: 500,
        };
        let results = search_paths(
            SearchScope::Workspace(&root),
            workspace_files(&root),
            &options,
            || true,
        )
        .expect("search should succeed");
        assert_eq!(results.len(), 1);
        assert!(results[0].0.ends_with("src/lib.rs"));
        assert_eq!(results[0].1[0].line, 0);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn replace_text_matches_preserves_literal_dollars_and_expands_regex_captures() {
        let literal = FileSearchOptions {
            pattern: "needle".into(),
            case_sensitive: true,
            whole_word: true,
            is_regex: false,
            max_results: 1,
        };
        assert_eq!(
            replace_text_matches(
                "needle needlessly needle_2 needle",
                &literal,
                "$1"
            )
            .expect("literal replacement"),
            ("$1 needlessly needle_2 $1".into(), 2)
        );

        let regex = FileSearchOptions {
            pattern: r"(?P<word>needle)".into(),
            case_sensitive: false,
            whole_word: true,
            is_regex: true,
            max_results: 1,
        };
        assert_eq!(
            replace_text_matches("Needle needlessly", &regex, r"<$word>\n")
                .expect("regex replacement"),
            ("<Needle>\n needlessly".into(), 1)
        );
    }

    #[test]
    fn replace_path_match_changes_only_the_selected_unsaved_match() {
        let workspace = tempfile::tempdir().expect("workspace");
        let path = workspace.path().join("main.rs");
        fs::write(&path, "needle needle\n").expect("write search file");
        let unsaved_text = "é needle needle\n";
        let overrides = HashMap::from([(path.clone(), unsaved_text.to_string())]);
        let options = FileSearchOptions {
            pattern: "needle".into(),
            case_sensitive: true,
            whole_word: false,
            is_regex: false,
            max_results: 10,
        };
        let matched = FileSearchMatch {
            line: 0,
            start: 10,
            end_line: 0,
            end: 16,
            line_content: unsaved_text.trim_end().into(),
            preview_match: 10..16,
        };

        let result = replace_path_match_with_overrides(
            SearchScope::Workspace(workspace.path()),
            &path,
            &overrides,
            &options,
            "done",
            &matched,
            || true,
        )
        .expect("replace selected match")
        .expect("selected result remains available");

        assert_eq!(result.match_count, 1);
        assert_eq!(result.original_text, unsaved_text);
        assert_eq!(result.replacement_text, "é needle done\n");
        assert_eq!(
            fs::read_to_string(path).expect("disk contents"),
            "needle needle\n"
        );
    }

    #[test]
    fn replace_selected_regex_match_expands_captures_and_escapes() {
        let options = FileSearchOptions {
            pattern: r"(?P<word>cat)(?P<plural>s?)".into(),
            case_sensitive: true,
            whole_word: false,
            is_regex: true,
            max_results: 10,
        };
        let matched = FileSearchMatch {
            line: 0,
            start: 0,
            end_line: 0,
            end: 4,
            line_content: "cats cat".into(),
            preview_match: 0..4,
        };

        assert_eq!(
            replace_text_match("cats cat", &options, "$word!$plural\\n", &matched)
                .expect("regex replacement")
                .expect("selected regex match"),
            "cat!s\n cat"
        );
    }

    #[test]
    fn replace_text_matches_rejects_invalid_regex_without_changing_text() {
        let options = FileSearchOptions {
            pattern: "(".into(),
            case_sensitive: false,
            whole_word: false,
            is_regex: true,
            max_results: 100,
        };
        assert!(matches!(
            replace_text_matches("unchanged", &options, "replacement"),
            Err(FileSearchError::InvalidPattern(_))
        ));
    }

    #[test]
    fn replace_paths_rejects_more_than_the_global_match_limit() {
        let first_path = PathBuf::from("first.txt");
        let second_path = PathBuf::from("second.txt");
        let mut overrides = HashMap::from([
            (
                first_path.clone(),
                "needle ".repeat(MAX_SEARCH_RESULT_RANGES / 2),
            ),
            (
                second_path.clone(),
                "needle ".repeat(MAX_SEARCH_RESULT_RANGES / 2),
            ),
        ]);
        let options = FileSearchOptions {
            pattern: "needle".into(),
            case_sensitive: true,
            whole_word: false,
            is_regex: false,
            max_results: 1,
        };

        let paths = [first_path, second_path.clone()];
        let replacements = replace_paths_with_overrides(
            SearchScope::BuffersOnly,
            paths.clone(),
            &overrides,
            &options,
            "changed",
            || true,
        )
        .expect("the exact global match limit should be accepted");
        assert_eq!(
            replacements
                .iter()
                .map(|replacement| replacement.match_count)
                .sum::<usize>(),
            MAX_SEARCH_RESULT_RANGES
        );

        overrides.insert(
            second_path,
            "needle ".repeat(MAX_SEARCH_RESULT_RANGES / 2 + 1),
        );
        assert_eq!(
            replace_paths_with_overrides(
                SearchScope::BuffersOnly,
                paths,
                &overrides,
                &options,
                "changed",
                || true,
            ),
            Err(FileSearchError::TooManyMatches)
        );
    }

    #[test]
    fn replace_paths_uses_unsaved_buffers_and_never_writes_files() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let workspace = temporary.path().join("workspace");
        fs::create_dir(&workspace).expect("create workspace");
        let saved_path = workspace.join("saved.txt");
        let new_path = workspace.join("new.txt");
        let outside_path = temporary.path().join("outside.txt");
        fs::write(&saved_path, "saved needle\n").expect("write saved file");
        fs::write(&outside_path, "outside needle\n").expect("write outside file");
        let overrides = HashMap::from([
            (saved_path.clone(), "unsaved needle needle\n".to_string()),
            (new_path.clone(), "new needle\n".to_string()),
            (
                outside_path.clone(),
                "outside override needle\n".to_string(),
            ),
        ]);
        let options = FileSearchOptions {
            pattern: "needle".into(),
            case_sensitive: true,
            whole_word: true,
            is_regex: false,
            max_results: 1,
        };

        let replacements = replace_paths_with_overrides(
            SearchScope::Workspace(&workspace),
            [
                saved_path.clone(),
                saved_path.clone(),
                new_path.clone(),
                outside_path,
            ],
            &overrides,
            &options,
            "changed",
            || true,
        )
        .expect("replace workspace text");

        assert_eq!(replacements.len(), 2);
        assert_eq!(replacements[0].path, saved_path);
        assert_eq!(replacements[0].original_text, "unsaved needle needle\n");
        assert_eq!(
            replacements[0].replacement_text,
            "unsaved changed changed\n"
        );
        assert_eq!(replacements[0].match_count, 2);
        assert_eq!(replacements[1].path, new_path);
        assert_eq!(replacements[1].match_count, 1);
        assert_eq!(
            fs::read_to_string(saved_path).expect("saved disk text"),
            "saved needle\n"
        );
    }

    #[test]
    fn stops_at_the_configured_match_limit() {
        let root = std::env::temp_dir()
            .join(format!("ahead-search-limit-{}", std::process::id()));
        fs::create_dir_all(&root).expect("create search directory");
        fs::write(root.join("matches.txt"), "needle needle\nneedle\n")
            .expect("write matches");
        let options = FileSearchOptions {
            pattern: "needle".to_string(),
            case_sensitive: true,
            whole_word: false,
            is_regex: false,
            max_results: 2,
        };

        let results = search_paths(
            SearchScope::Workspace(&root),
            workspace_files(&root),
            &options,
            || true,
        )
        .expect("search should succeed");
        assert_eq!(
            results
                .iter()
                .map(|(_, matches)| matches.len())
                .sum::<usize>(),
            2
        );
        assert_eq!(results[0].1[0].line, 0);
        assert_eq!(results[0].1[0].start, 0);
        assert_eq!(results[0].1[1].line, 0);
        assert_eq!(results[0].1[1].start, 7);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn streams_matching_files_and_stops_when_receiver_closes() {
        let root = std::env::temp_dir()
            .join(format!("ahead-search-stream-{}", std::process::id()));
        fs::create_dir_all(&root).expect("create search directory");
        fs::write(root.join("a.txt"), "needle\n").expect("write first file");
        fs::write(root.join("b.txt"), "needle\n").expect("write second file");
        let options = FileSearchOptions {
            pattern: "needle".to_string(),
            case_sensitive: true,
            whole_word: false,
            is_regex: false,
            max_results: 10,
        };
        let mut delivered = Vec::new();

        let result = search_paths_with_overrides_stream(
            SearchScope::Workspace(&root),
            workspace_paths(&root),
            &HashMap::new(),
            &options,
            || true,
            |path, matches| {
                delivered.push((path, matches));
                false
            },
        );
        assert_eq!(result, Err(FileSearchError::Cancelled));
        assert_eq!(delivered.len(), 1);
        assert!(delivered[0].0.ends_with("a.txt"));
        assert_eq!(delivered[0].1[0].line, 0);
        fs::remove_dir_all(root).expect("remove search directory");
    }

    #[test]
    fn cancelled_index_walk_can_be_rebuilt() {
        let root = std::env::temp_dir()
            .join(format!("ahead-search-index-cancel-{}", std::process::id()));
        fs::create_dir_all(&root).expect("create search directory");
        fs::write(root.join("a.rs"), "first\n").expect("write first file");
        fs::write(root.join("b.rs"), "second\n").expect("write second file");
        let index = WorkspaceFileIndex::new(root.clone());
        let checks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let walk_checks = checks.clone();

        assert!(
            index
                .snapshot_with_generation_while(move || {
                    walk_checks.fetch_add(1, Ordering::SeqCst) < 3
                })
                .is_none()
        );
        assert_eq!(index.snapshot().len(), 2);
        fs::remove_dir_all(root).expect("remove search directory");
    }

    #[test]
    fn stream_reports_when_results_hit_the_limit() {
        let root = std::env::temp_dir()
            .join(format!("ahead-search-stream-limit-{}", std::process::id()));
        fs::create_dir_all(&root).expect("create search directory");
        fs::write(root.join("matches.txt"), "needle needle\n")
            .expect("write matches");
        let options = FileSearchOptions {
            pattern: "needle".to_string(),
            case_sensitive: true,
            whole_word: false,
            is_regex: false,
            max_results: 1,
        };
        let mut delivered = Vec::new();

        let limit_reached = search_paths_with_overrides_stream(
            SearchScope::Workspace(&root),
            workspace_paths(&root),
            &HashMap::new(),
            &options,
            || true,
            |path, matches| {
                delivered.push((path, matches));
                true
            },
        )
        .expect("search succeeds");
        assert!(limit_reached);
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0].1.len(), 1);
        fs::remove_dir_all(root).expect("remove search directory");
    }

    #[test]
    fn parallel_stream_preserves_input_order_and_global_match_limit() {
        let workspace = tempfile::tempdir().expect("workspace");
        let first = workspace.path().join("z-first.txt");
        let second = workspace.path().join("a-second.txt");
        let mut first_content = "ordinary line without a match\n".repeat(100_000);
        first_content.push_str("needle one\nneedle two\n");
        fs::write(&first, first_content).expect("write first file");
        fs::write(&second, "needle three\nneedle four\n")
            .expect("write second file");
        let mut paths = vec![first.clone()];
        for index in 0..SEARCH_PATHS_PER_JOB - 1 {
            let path = workspace.path().join(format!("middle-{index}.txt"));
            fs::write(&path, "no match\n").expect("write middle file");
            paths.push(path);
        }
        paths.push(second.clone());
        let options = FileSearchOptions {
            pattern: "needle".to_string(),
            case_sensitive: true,
            whole_word: false,
            is_regex: false,
            max_results: 3,
        };
        let mut delivered = Vec::new();

        let limit_reached = search_paths_with_overrides_stream(
            SearchScope::Workspace(workspace.path()),
            paths,
            &HashMap::new(),
            &options,
            || true,
            |path, matches| {
                delivered.push((path, matches));
                true
            },
        )
        .expect("search succeeds");

        assert!(limit_reached);
        assert_eq!(delivered.len(), 2);
        assert_eq!(delivered[0].0, first);
        assert_eq!(delivered[0].1.len(), 2);
        assert!(delivered[1].0.ends_with("a-second.txt"));
        assert_eq!(delivered[1].1.len(), 1);
    }

    #[test]
    fn parallel_stream_cancellation_stops_feeding_the_bounded_worker_window() {
        use std::sync::atomic::AtomicUsize;

        let workspace = tempfile::tempdir().expect("workspace");
        let content = "needle\n".repeat(1_000);
        let paths = (0..64)
            .map(|index| {
                let path = workspace.path().join(format!("file-{index:02}.txt"));
                fs::write(&path, &content).expect("write search fixture");
                path
            })
            .collect::<Vec<_>>();
        let yielded = AtomicUsize::new(0);
        let options = FileSearchOptions {
            pattern: "needle".to_string(),
            case_sensitive: true,
            whole_word: false,
            is_regex: false,
            max_results: 10_000,
        };
        let mut delivered = 0;
        let result = search_paths_with_overrides_stream(
            SearchScope::Workspace(workspace.path()),
            paths.into_iter().inspect(|_| {
                yielded.fetch_add(1, Ordering::Relaxed);
            }),
            &HashMap::new(),
            &options,
            || true,
            |_, _| {
                delivered += 1;
                false
            },
        );

        assert_eq!(result, Err(FileSearchError::Cancelled));
        assert_eq!(delivered, 1);
        assert!(
            yielded.load(Ordering::Relaxed)
                <= search_worker_count().saturating_mul(SEARCH_PATHS_PER_JOB),
            "cancellation consumed {} paths before stopping",
            yielded.load(Ordering::Relaxed)
        );
    }

    #[test]
    fn searches_unsaved_content_instead_of_disk_content() {
        let root = std::env::temp_dir()
            .join(format!("ahead-search-overlay-{}", std::process::id()));
        fs::create_dir_all(&root).expect("create search directory");
        let path = root.join("buffer.rs");
        fs::write(&path, "old disk content\n").expect("write disk content");
        let options = FileSearchOptions {
            pattern: "unsaved".to_string(),
            case_sensitive: true,
            whole_word: false,
            is_regex: false,
            max_results: 10,
        };
        let overrides =
            HashMap::from([(path.clone(), "unsaved unsaved\n".to_string())]);

        let results = search_paths_with_overrides(
            SearchScope::Workspace(&root),
            workspace_files(&root),
            &overrides,
            &options,
            || true,
        )
        .expect("search should succeed");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, path);
        assert_eq!(results[0].1.len(), 2);
        assert_eq!(results[0].1[0].start, 0);
        assert_eq!(results[0].1[1].start, 8);
        fs::remove_dir_all(root).expect("remove search directory");
    }

    #[test]
    fn agent_paths_exclude_private_files_and_are_sorted() {
        let temporary = std::env::temp_dir()
            .join(format!("ahead-search-private-{}", std::process::id()));
        let root = temporary.join("server.pem/repo");
        fs::create_dir_all(&root).expect("create search directory");
        for name in ["z.rs", "a.rs", "server.pem", "secrets.yml"] {
            fs::write(root.join(name), "needle\n").expect("write search file");
        }

        let files = agent_workspace_paths(&root)
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(files, ["a.rs", "z.rs"]);
        let indexed_files = WorkspaceFileIndex::new(root.clone())
            .snapshot()
            .iter()
            .filter(|path| is_agent_visible_path(&root, path))
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(indexed_files, files);
        assert!(is_private_file(Path::new(".ahead/settings.toml")));
        assert!(is_private_file(Path::new("src/.ahead/session.db")));
        assert!(!is_private_file(Path::new(
            ".agents/skills/search/SKILL.md"
        )));
        fs::remove_dir_all(temporary).expect("remove search directory");
    }

    #[test]
    fn skips_binary_files_with_nul_bytes() {
        let root = std::env::temp_dir()
            .join(format!("ahead-search-binary-{}", std::process::id()));
        fs::create_dir_all(&root).expect("create search directory");
        let path = root.join("binary.dat");
        fs::write(&path, b"needle\0binary\n").expect("write binary file");
        let options = FileSearchOptions {
            pattern: "needle".to_string(),
            case_sensitive: true,
            whole_word: false,
            is_regex: false,
            max_results: 10,
        };

        let results = search_paths(
            SearchScope::Workspace(&root),
            std::iter::once(path),
            &options,
            || true,
        )
        .expect("search should succeed");
        assert!(results.is_empty());
        fs::remove_dir_all(root).expect("remove search directory");
    }

    #[test]
    fn decodes_unmarked_utf16_and_skips_binary_content() {
        let root = std::env::temp_dir()
            .join(format!("ahead-search-byte-class-{}", std::process::id()));
        fs::create_dir_all(&root).expect("create search directory");
        let utf16_le = root.join("utf16-le.txt");
        let utf16_be = root.join("utf16-be.txt");
        let utf16_cyrillic = root.join("utf16-cyrillic.txt");
        let text = "needle\n";
        fs::write(
            &utf16_le,
            text.encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>(),
        )
        .expect("write BOM-less UTF-16LE text");
        fs::write(
            &utf16_be,
            text.encode_utf16()
                .flat_map(u16::to_be_bytes)
                .collect::<Vec<_>>(),
        )
        .expect("write BOM-less UTF-16BE text");
        let cyrillic_text = "Привет, needle\n";
        fs::write(
            &utf16_cyrillic,
            cyrillic_text
                .encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>(),
        )
        .expect("write BOM-less UTF-16LE Cyrillic text");

        let control_heavy_binary = root.join("control-heavy.bin");
        let mut binary = b"needle\n".to_vec();
        binary.extend([0x01; 128]);
        fs::write(&control_heavy_binary, binary)
            .expect("write no-NUL binary content");
        let late_binary = root.join("late-binary.bin");
        let mut late_binary_content = b"needle\n".to_vec();
        late_binary_content.resize(SEARCH_PREFIX_BYTES, b'x');
        late_binary_content.extend([0x01; 128]);
        fs::write(&late_binary, late_binary_content)
            .expect("write text prefix followed by no-NUL binary content");
        let pdf = root.join("document.pdf");
        fs::write(&pdf, b"%PDF-1.7\nneedle\n").expect("write PDF signature");
        let jpeg = root.join("image.jpg");
        fs::write(&jpeg, b"\xFF\xD8\xFFneedle").expect("write JPEG signature");
        let bzip2 = root.join("archive.bz2");
        fs::write(&bzip2, b"BZh9needle\n").expect("write printable BZip2 signature");
        let zstd = root.join("archive.zst");
        fs::write(&zstd, b"\x28\xB5\x2F\xFDneedle\n")
            .expect("write Zstandard signature");

        let options = FileSearchOptions {
            pattern: "needle".to_string(),
            case_sensitive: true,
            whole_word: false,
            is_regex: false,
            max_results: 10,
        };
        let results = search_paths(
            SearchScope::Workspace(&root),
            [
                utf16_le.clone(),
                utf16_be.clone(),
                utf16_cyrillic.clone(),
                control_heavy_binary,
                late_binary,
                pdf,
                jpeg,
                bzip2,
                zstd,
            ],
            &options,
            || true,
        )
        .expect("encoded text search should succeed");
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].0, utf16_le);
        assert_eq!(results[1].0, utf16_be);
        assert_eq!(results[2].0, utf16_cyrillic);
        for (_, matches) in results.iter().take(2) {
            assert_eq!(matches.len(), 1);
            assert_eq!(matches[0].line, 0);
            assert_eq!(matches[0].start, 0);
            assert_eq!(matches[0].end, text.trim_end().len());
            assert_eq!(matches[0].line_content, text.trim_end());
        }
        assert_eq!(results[2].1[0].start, "Привет, ".len());
        assert_eq!(results[2].1[0].end, "Привет, needle".len());
        assert_eq!(results[2].1[0].line_content, "Привет, needle");
        fs::remove_dir_all(root).expect("remove search directory");
    }

    #[test]
    fn recognizes_common_compressed_file_signatures() {
        let signatures: &[(&str, &[u8])] = &[
            ("gzip", b"\x1F\x8B\x08"),
            ("BZip2", b"BZh9block"),
            ("XZ", b"\xFD7zXZ\x00"),
            ("Zstandard", b"\x28\xB5\x2F\xFD"),
            ("LZ4", b"\x04\x22\x4D\x18"),
            ("7z", b"7z\xBC\xAF\x27\x1C"),
            ("RAR", b"Rar!\x1A\x07"),
        ];
        for (format, signature) in signatures {
            assert!(is_binary_content(signature), "{format} signature");
        }

        assert!(!is_binary_content(b"BZh is a plain-text note"));
    }

    #[test]
    fn decodes_bom_and_detected_legacy_text_with_utf8_byte_columns() {
        let root = std::env::temp_dir()
            .join(format!("ahead-search-decode-{}", std::process::id()));
        fs::create_dir_all(&root).expect("create search directory");
        let legacy = root.join("windows-1251.txt");
        let utf16 = root.join("utf16.txt");
        let legacy_text = "строка один\nстрока два\n";
        let (legacy_bytes, _, _) = encoding_rs::WINDOWS_1251.encode(legacy_text);
        fs::write(&legacy, legacy_bytes.as_ref())
            .expect("write Windows-1251 search text");
        let mut utf16_file = vec![0xFF, 0xFE];
        utf16_file.extend("needle\n".encode_utf16().flat_map(u16::to_le_bytes));
        fs::write(&utf16, utf16_file).expect("write BOM-marked UTF-16 text");

        let legacy_options = FileSearchOptions {
            pattern: "строка".to_string(),
            case_sensitive: true,
            whole_word: false,
            is_regex: false,
            max_results: 10,
        };
        let legacy_results = search_paths(
            SearchScope::Workspace(&root),
            std::iter::once(legacy.clone()),
            &legacy_options,
            || true,
        )
        .expect("legacy text search should succeed");
        assert_eq!(legacy_results.len(), 1);
        assert_eq!(legacy_results[0].1.len(), 2);
        assert_eq!(legacy_results[0].1[0].line, 0,);
        assert_eq!(legacy_results[0].1[0].start, 0);
        assert_eq!(legacy_results[0].1[0].end, "строка".len());

        let partial = root.join("partial.txt");
        fs::write(&partial, b"needle\nneedle\xff\n")
            .expect("write text with a malformed UTF-8 byte");
        let partial_then_invalid_options = FileSearchOptions {
            pattern: "needle".to_string(),
            case_sensitive: true,
            whole_word: false,
            is_regex: false,
            max_results: 10,
        };
        let legacy_results = search_paths(
            SearchScope::Workspace(&root),
            std::iter::once(partial),
            &partial_then_invalid_options,
            || true,
        )
        .expect("search should recover matches before and after invalid UTF-8");
        assert_eq!(legacy_results[0].1.len(), 2);
        assert_eq!(legacy_results[0].1[0].line, 0);
        assert_eq!(legacy_results[0].1[1].line, 1);
        assert_eq!(legacy_results[0].1[1].start, 0);

        let utf16_options = FileSearchOptions {
            pattern: "needle".to_string(),
            case_sensitive: true,
            whole_word: false,
            is_regex: false,
            max_results: 10,
        };
        let utf16_results = search_paths(
            SearchScope::Workspace(&root),
            std::iter::once(utf16.clone()),
            &utf16_options,
            || true,
        )
        .expect("UTF-16 BOM search should succeed");
        assert_eq!(utf16_results.len(), 1);
        assert_eq!(utf16_results[0].0, utf16);
        assert_eq!(utf16_results[0].1[0].start, 0);
        assert_eq!(utf16_results[0].1[0].end, "needle".len());
        fs::remove_dir_all(root).expect("remove search directory");
    }

    #[test]
    fn cancelling_a_non_utf8_fallback_stops_the_decode_read() {
        let root = std::env::temp_dir()
            .join(format!("ahead-search-decode-cancel-{}", std::process::id()));
        fs::create_dir_all(&root).expect("create search directory");
        let path = root.join("legacy.txt");
        fs::write(&path, [0xFF; 256 * 1024]).expect("write non-UTF-8 file");
        let options = FileSearchOptions {
            pattern: "not present".to_string(),
            case_sensitive: true,
            whole_word: false,
            is_regex: false,
            max_results: 10,
        };
        let checks = std::sync::atomic::AtomicUsize::new(0);
        let result = search_paths(
            SearchScope::Workspace(&root),
            std::iter::once(path),
            &options,
            || checks.fetch_add(1, Ordering::Relaxed) < 2,
        );
        assert_eq!(result, Err(FileSearchError::Cancelled));
        fs::remove_dir_all(root).expect("remove search directory");
    }

    #[test]
    fn empty_query_does_not_walk_or_match_files() {
        let options = FileSearchOptions {
            pattern: String::new(),
            case_sensitive: false,
            whole_word: false,
            is_regex: false,
            max_results: 10,
        };
        let paths = std::iter::from_fn(|| panic!("empty query walked files"));
        let results =
            search_paths(SearchScope::BuffersOnly, paths, &options, || true)
                .expect("empty query should succeed");
        assert!(results.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn indexed_search_path_cannot_follow_replaced_file_or_directory() {
        use std::os::unix::fs::symlink;

        let temporary = tempfile::tempdir().expect("disposable search project");
        let workspace = temporary.path().join("workspace");
        let outside = temporary.path().join("outside");
        fs::create_dir_all(workspace.join("src")).expect("create source directory");
        fs::create_dir_all(&outside).expect("create outside directory");
        let indexed = workspace.join("src/file.rs");
        let external = outside.join("file.rs");
        fs::write(&indexed, "public\n").expect("write indexed file");
        fs::write(&external, "secret-needle\n").expect("write outside file");
        let paths = workspace_paths(&workspace).collect::<Vec<_>>();
        let options = FileSearchOptions {
            pattern: "secret-needle".into(),
            case_sensitive: true,
            whole_word: false,
            is_regex: false,
            max_results: 10,
        };

        fs::remove_file(&indexed).expect("remove indexed file");
        symlink(&external, &indexed).expect("replace indexed file with symlink");
        assert!(
            search_paths(
                SearchScope::Workspace(&workspace),
                paths.clone(),
                &options,
                || true
            )
            .expect("search file replacement")
            .is_empty()
        );

        fs::remove_file(&indexed).expect("remove file symlink");
        fs::remove_dir(workspace.join("src")).expect("remove source directory");
        symlink(&outside, workspace.join("src"))
            .expect("replace directory with symlink");
        assert!(
            search_paths(
                SearchScope::Workspace(&workspace),
                paths,
                &options,
                || true
            )
            .expect("search directory replacement")
            .is_empty()
        );

        let outside_buffers =
            HashMap::from([(external.clone(), "secret-needle".to_string())]);
        assert!(
            search_paths_with_overrides(
                SearchScope::Workspace(&workspace),
                std::iter::once(external),
                &outside_buffers,
                &options,
                || true,
            )
            .expect("search outside override")
            .is_empty()
        );
    }

    #[test]
    fn resolves_only_workspace_open_buffers() {
        let temporary = std::env::temp_dir()
            .join(format!("ahead-search-open-{}", std::process::id()));
        let workspace = temporary.join("workspace");
        fs::create_dir_all(workspace.join("src"))
            .expect("create workspace source directory");
        fs::write(workspace.join("src/lib.rs"), "saved\n")
            .expect("write saved file");

        assert_eq!(
            resolve_open_buffer_path(&workspace, Path::new("src/lib.rs")),
            Some(workspace.join("src/lib.rs"))
        );
        assert_eq!(
            resolve_open_buffer_path(&workspace, Path::new("src/new.rs")),
            Some(workspace.join("src/new.rs"))
        );
        assert!(
            resolve_open_buffer_path(&workspace, Path::new("../outside.rs"))
                .is_none()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let outside = temporary.join("outside.rs");
            fs::write(&outside, "private\n").expect("write outside file");
            symlink(&outside, workspace.join("src/escape.rs"))
                .expect("create symlink escape");
            assert!(
                resolve_open_buffer_path(&workspace, Path::new("src/escape.rs"))
                    .is_none()
            );
            symlink(workspace.join("src"), workspace.join("linked-src"))
                .expect("create directory symlink");
            assert!(
                resolve_open_buffer_path(&workspace, Path::new("linked-src/lib.rs"))
                    .is_none()
            );
        }
        fs::remove_dir_all(temporary).expect("remove search directory");
    }

    #[test]
    fn new_open_paths_are_sorted_and_workspace_bound() {
        let temporary = std::env::temp_dir()
            .join(format!("ahead-search-new-open-{}", std::process::id()));
        let workspace = temporary.join("workspace");
        fs::create_dir_all(workspace.join("src")).expect("create source directory");
        fs::write(workspace.join("src/existing.rs"), "saved\n")
            .expect("write existing file");
        let overrides = HashMap::from([
            (workspace.join("src/z.rs"), "new".to_string()),
            (workspace.join("src/a.rs"), "new".to_string()),
            (workspace.join("src/existing.rs"), "changed".to_string()),
            (temporary.join("outside.rs"), "outside".to_string()),
            (workspace.join("../escape.rs"), "outside".to_string()),
        ]);

        assert_eq!(
            new_open_buffer_paths(&workspace, &overrides),
            [workspace.join("src/a.rs"), workspace.join("src/z.rs")]
        );
        fs::remove_dir_all(temporary).expect("remove search directory");
    }

    #[test]
    fn path_filter_matches_relative_and_named_worktree_globs() {
        let workspace = Path::new("/projects/ahead");
        let source = workspace.join("src/lib.rs");
        let other = workspace.join("docs/readme.md");
        let filter =
            WorkspacePathFilter::new(workspace, Some(" src/**/*.rs "), None)
                .expect("valid include glob");
        assert!(filter.matches(&source));
        assert!(!filter.matches(&other));
        assert!(!filter.matches(Path::new("/projects/other/src/lib.rs")));

        let named =
            WorkspacePathFilter::new(workspace, Some("ahead/src/**/*.rs"), None)
                .expect("valid named worktree glob");
        assert!(named.matches(&source));
    }

    #[test]
    fn path_filter_exclusion_wins_and_rejects_invalid_globs() {
        let workspace = Path::new("/projects/ahead");
        let filter = WorkspacePathFilter::new(
            workspace,
            Some("**/*.rs"),
            Some("src/generated/**"),
        )
        .expect("valid globs");
        assert!(filter.matches(&workspace.join("src/lib.rs")));
        assert!(!filter.matches(&workspace.join("src/generated/code.rs")));
        assert!(matches!(
            WorkspacePathFilter::new(workspace, Some("["), None),
            Err(FileSearchError::InvalidGlob(_))
        ));
    }

    #[test]
    fn path_filter_accepts_multiple_patterns_and_literal_directories() {
        let workspace = Path::new("/projects/ahead");
        let filter = WorkspacePathFilter::new(
            workspace,
            Some(" src , {docs,tests}/**/*.md "),
            Some("src/generated, docs/draft.md"),
        )
        .expect("valid path patterns");
        assert!(filter.matches(&workspace.join("src/lib.rs")));
        assert!(filter.matches(&workspace.join("docs/readme.md")));
        assert!(filter.matches(&workspace.join("tests/guide.md")));
        assert!(!filter.matches(&workspace.join("src/generated/code.rs")));
        assert!(!filter.matches(&workspace.join("docs/draft.md")));
        assert!(!filter.matches(&workspace.join("notes/readme.md")));
    }

    #[test]
    fn path_filter_keeps_commas_inside_glob_character_classes() {
        let workspace = Path::new("/projects/ahead");
        let filter = WorkspacePathFilter::new(
            workspace,
            Some("src/[a,b].rs,docs/*.md"),
            None,
        )
        .expect("valid patterns with a comma inside a character class");

        assert!(filter.matches(&workspace.join("src/a.rs")));
        assert!(filter.matches(&workspace.join("src/b.rs")));
        assert!(filter.matches(&workspace.join("docs/readme.md")));
        assert!(!filter.matches(&workspace.join("src/c.rs")));
    }

    #[test]
    fn supports_case_word_and_regex_modes() {
        let workspace = std::env::temp_dir()
            .join(format!("ahead-search-modes-{}", std::process::id()));
        fs::create_dir_all(&workspace).expect("create workspace");
        let path = workspace.join("example.txt");
        fs::write(&path, "Cat cat catalog\r\ncat\r\ncat\n").expect("write source");
        let mut options = FileSearchOptions {
            pattern: "cat".into(),
            case_sensitive: false,
            whole_word: false,
            is_regex: false,
            max_results: 10,
        };
        let count = |options: &FileSearchOptions| {
            search_paths(
                SearchScope::Workspace(&workspace),
                std::iter::once(path.clone()),
                options,
                || true,
            )
            .expect("search should succeed")
            .into_iter()
            .map(|(_, matches)| matches.len())
            .sum::<usize>()
        };
        assert_eq!(count(&options), 5);
        options.case_sensitive = true;
        assert_eq!(count(&options), 4);
        options.whole_word = true;
        assert_eq!(count(&options), 3);
        options.whole_word = false;
        options.is_regex = true;
        assert_eq!(count(&options), 4);
        options.whole_word = true;
        assert_eq!(count(&options), 3);
        options.pattern = "^cat$".into();
        options.whole_word = false;
        assert_eq!(count(&options), 2);
        options.whole_word = true;
        assert_eq!(count(&options), 2);
        options.pattern = "[".into();
        assert!(matches!(
            search_paths(
                SearchScope::Workspace(&workspace),
                std::iter::once(path),
                &options,
                || true
            ),
            Err(FileSearchError::InvalidPattern(_))
        ));
        fs::remove_dir_all(workspace).expect("remove workspace");
    }

    #[test]
    fn regex_search_reports_multiline_match_range() {
        let workspace = std::env::temp_dir()
            .join(format!("ahead-search-multiline-{}", std::process::id()));
        fs::create_dir_all(&workspace).expect("create workspace");
        let path = workspace.join("example.txt");
        fs::write(&path, "before\n  first\nsecond tail\n").expect("write source");
        let options = FileSearchOptions {
            pattern: r"first\nsecond".to_string(),
            case_sensitive: true,
            whole_word: false,
            is_regex: true,
            max_results: 10,
        };

        let results = search_paths(
            SearchScope::Workspace(&workspace),
            std::iter::once(path),
            &options,
            || true,
        )
        .expect("search should succeed");
        let matches = &results.first().expect("file should match").1;
        assert_eq!(matches.len(), 1);
        let matched = matches.first().expect("multiline regex should match");
        assert_eq!(matched.line, 1);
        assert_eq!(matched.start, 2);
        assert_eq!(matched.end_line, 2);
        assert_eq!(matched.end, 6);
        assert_eq!(matched.line_content, "  first");
        assert_eq!(matched.preview_match, 2..7);
        assert_eq!(
            matched.line_content.get(matched.preview_match.clone()),
            Some("first")
        );
        fs::remove_dir_all(workspace).expect("remove workspace");
    }

    #[test]
    fn regex_search_supports_lookaround_and_replacement() {
        let workspace = std::env::temp_dir()
            .join(format!("ahead-search-lookaround-{}", std::process::id()));
        fs::create_dir_all(&workspace).expect("create workspace");
        let path = workspace.join("example.txt");
        fs::write(&path, "foobarbaz\n").expect("write source");
        let options = FileSearchOptions {
            pattern: r"(?<=foo)bar(?=baz)".into(),
            case_sensitive: true,
            whole_word: false,
            is_regex: true,
            max_results: 10,
        };

        let results = search_paths(
            SearchScope::Workspace(&workspace),
            std::iter::once(path),
            &options,
            || true,
        )
        .expect("lookaround regex should search successfully");
        let matches = &results.first().expect("file should match").1;
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].start..matches[0].end, 3..6);
        assert_eq!(matches[0].line_content, "foobarbaz");
        assert_eq!(
            replace_text_matches("foobarbaz", &options, "qux")
                .expect("lookaround regex should replace successfully"),
            ("fooquxbaz".into(), 1)
        );
        fs::remove_dir_all(workspace).expect("remove search directory");
    }

    #[test]
    fn literal_search_reports_multiline_match_range() {
        let workspace = std::env::temp_dir().join(format!(
            "ahead-search-multiline-literal-{}",
            std::process::id()
        ));
        fs::create_dir_all(&workspace).expect("create workspace");
        let path = workspace.join("example.txt");
        fs::write(&path, "before\n  first\nsecond tail\n").expect("write source");
        let options = FileSearchOptions {
            pattern: "first\nsecond".to_string(),
            case_sensitive: true,
            whole_word: false,
            is_regex: false,
            max_results: 10,
        };

        let results = search_paths(
            SearchScope::Workspace(&workspace),
            std::iter::once(path),
            &options,
            || true,
        )
        .expect("search should succeed");
        let matches = &results.first().expect("file should match").1;
        assert_eq!(matches.len(), 1);
        let matched = matches.first().expect("multiline literal should match");
        assert_eq!(matched.line, 1);
        assert_eq!(matched.start, 2);
        assert_eq!(matched.end_line, 2);
        assert_eq!(matched.end, 6);
        fs::remove_dir_all(workspace).expect("remove workspace");
    }

    #[test]
    fn clipped_unicode_preview_keeps_match_range_in_utf8_bytes() {
        let root = std::env::temp_dir().join(format!(
            "ahead-search-preview-unicode-{}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("create search directory");
        let path = root.join("long.txt");
        fs::write(&path, format!("{}needle tail", "é".repeat(150)))
            .expect("write Unicode line");
        let options = FileSearchOptions {
            pattern: "needle".to_string(),
            case_sensitive: true,
            whole_word: false,
            is_regex: false,
            max_results: 10,
        };

        let results = search_paths(
            SearchScope::Workspace(&root),
            std::iter::once(path),
            &options,
            || true,
        )
        .expect("search should succeed");
        let matched = &results[0].1[0];
        assert_eq!(
            matched.line_content.get(matched.preview_match.clone()),
            Some("needle")
        );
        assert!(matched.line_content.len() <= 400);
        fs::remove_dir_all(root).expect("remove search directory");
    }

    #[test]
    fn workspace_file_index_rebuilds_after_invalidation() {
        let workspace = std::env::temp_dir()
            .join(format!("ahead-search-index-{}", std::process::id()));
        fs::create_dir_all(&workspace).expect("create workspace");
        fs::write(workspace.join("a.rs"), "a").expect("write first file");
        let index = WorkspaceFileIndex::new(workspace.clone());
        let (first_generation, first) = index.snapshot_with_generation();
        assert_eq!(first_generation, 0);
        assert_eq!(first.as_slice(), &[workspace.join("a.rs")]);

        let nested = workspace.join("nested");
        fs::create_dir(&nested).expect("create nested directory");
        assert!(index.affects_search(&nested, true));
        assert!(Arc::ptr_eq(&first, &index.snapshot()));
        index.invalidate();
        assert!(
            index.affects_search(&nested.join("b.rs"), true),
            "an invalidated entry snapshot must not hide new nested files"
        );
        fs::write(nested.join("b.rs"), "b").expect("write second file");
        assert_eq!(index.generation(), 1);
        let (second_generation, second) = index.snapshot_with_generation();
        assert_eq!(second_generation, 1);
        assert!(!Arc::ptr_eq(&first, &second));
        assert_eq!(
            second.as_slice(),
            &[workspace.join("a.rs"), nested.join("b.rs")]
        );
        fs::remove_dir_all(workspace).expect("remove workspace");
    }

    #[test]
    fn workspace_file_index_releases_cancelled_cold_build_for_retry() {
        let workspace = tempfile::tempdir().expect("disposable workspace");
        for number in 0..4 {
            fs::write(workspace.path().join(format!("{number}.rs")), "source")
                .expect("write source file");
        }
        let index = WorkspaceFileIndex::new(workspace.path().to_path_buf());
        let checks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let walk_checks = checks.clone();
        assert!(
            index
                .snapshot_with_generation_while(move || {
                    walk_checks.fetch_add(1, Ordering::SeqCst) < 3
                })
                .is_none(),
            "a search should retry when a superseded cold scan releases ownership"
        );
        assert!(checks.load(Ordering::SeqCst) >= 3);
        assert_eq!(index.snapshot().len(), 4);
    }

    #[test]
    fn workspace_file_index_coalesces_invalidations_without_a_snapshot() {
        let workspace = tempfile::tempdir().expect("disposable workspace");
        let index = WorkspaceFileIndex::new(workspace.path().to_path_buf());
        let first_revision = index.search_revision();
        index.invalidate();
        index.invalidate();
        assert_eq!(index.generation(), 0);
        assert_eq!(index.search_revision(), first_revision);

        let first_file = workspace.path().join("first.rs");
        fs::write(&first_file, "first").expect("write first file");
        assert_eq!(
            index.snapshot().as_slice(),
            std::slice::from_ref(&first_file)
        );

        index.invalidate();
        let second_revision = index.search_revision();
        index.invalidate();
        assert_eq!(index.generation(), 1);
        assert_eq!(index.search_revision(), second_revision);

        let second_file = workspace.path().join("second.rs");
        fs::write(&second_file, "second").expect("write second file");
        assert_eq!(index.snapshot().as_slice(), &[first_file, second_file]);
    }

    #[test]
    fn workspace_content_revision_keeps_the_cached_path_snapshot() {
        let workspace = std::env::temp_dir().join(format!(
            "ahead-search-content-revision-{}",
            std::process::id()
        ));
        fs::create_dir_all(&workspace).expect("create workspace");
        let file = workspace.join("a.rs");
        fs::write(&file, "needle").expect("write file");
        let index = WorkspaceFileIndex::new(workspace.clone());
        let (generation, search_revision, first) = index
            .snapshot_with_search_revision_while(|| true)
            .expect("initial snapshot");
        assert_eq!(generation, 0);
        assert_eq!(search_revision, index.search_revision());
        assert_ne!(
            search_revision,
            WorkspaceFileIndex::new(workspace.clone()).search_revision()
        );

        index.mark_content_changed();

        let (next_generation, next_revision, next) = index
            .snapshot_with_search_revision_while(|| true)
            .expect("content-updated snapshot");
        assert_eq!(next_generation, generation);
        assert_eq!(next_revision, search_revision + 1);
        assert!(Arc::ptr_eq(&first, &next));
        fs::remove_dir_all(workspace).expect("remove workspace");
    }

    #[test]
    fn quick_open_ranks_paths_relative_to_the_current_directory() {
        let workspace = Path::new("/workspace");
        let paths = vec![
            workspace.join("src/agent.rs"),
            workspace.join("lib/agent.rs"),
            PathBuf::from("/other/agent.rs"),
        ];

        let matches = rank_file_paths(
            workspace,
            paths,
            "agent.rs",
            Some(Path::new("lib")),
            10,
            || true,
        )
        .expect("path ranking should succeed");

        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].relative_path, "lib/agent.rs");
        assert_eq!(matches[0].distance_to_relative_directory, 2);
        assert!(!matches[0].positions.is_empty());
        assert!(matches.iter().all(|matched| {
            matched
                .positions
                .iter()
                .all(|position| *position < matched.relative_path.chars().count())
        }));
    }

    #[test]
    fn quick_open_smart_case_prefers_matching_path_capitalization() {
        let workspace = Path::new("/workspace");
        let paths = vec![
            workspace.join("src/agent.rs"),
            workspace.join("src/Agent.rs"),
        ];

        let matches = rank_file_paths(workspace, paths, "Agent", None, 10, || true)
            .expect("path ranking should succeed");

        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].relative_path, "src/Agent.rs");
        assert!(matches[0].score > matches[1].score);
    }

    #[test]
    fn quick_open_cancels_a_superseded_rank_request() {
        let paths = vec![PathBuf::from("/workspace/src/agent.rs")];
        let result = rank_file_paths(
            Path::new("/workspace"),
            paths,
            "agent",
            None,
            10,
            || false,
        );

        assert_eq!(result, Err(FileSearchError::Cancelled));
    }

    #[test]
    fn concurrent_snapshot_callers_share_one_cached_view() {
        use std::sync::Barrier;

        let workspace = std::env::temp_dir().join(format!(
            "ahead-search-concurrent-index-{}",
            std::process::id()
        ));
        fs::create_dir_all(&workspace).expect("create workspace");
        fs::write(workspace.join("a.rs"), "a").expect("write file");
        let index = Arc::new(WorkspaceFileIndex::new(workspace.clone()));
        let start = Arc::new(Barrier::new(9));
        std::thread::scope(|scope| {
            let workers = (0..8)
                .map(|_| {
                    let index = index.clone();
                    let start = start.clone();
                    scope.spawn(move || {
                        start.wait();
                        index.snapshot_with_generation()
                    })
                })
                .collect::<Vec<_>>();
            start.wait();
            let mut workers = workers.into_iter();
            let first = workers
                .next()
                .expect("first snapshot worker")
                .join()
                .expect("first snapshot worker");
            assert_eq!(first.0, 0);
            assert_eq!(first.1.as_slice(), &[workspace.join("a.rs")]);
            for worker in workers {
                let other = worker.join().expect("snapshot worker");
                assert_eq!(other.0, first.0);
                assert!(Arc::ptr_eq(&other.1, &first.1));
            }
        });
        fs::remove_dir_all(workspace).expect("remove workspace");
    }

    #[test]
    fn limits_preview_size_even_when_match_covers_the_line() {
        let root = std::env::temp_dir()
            .join(format!("ahead-search-preview-{}", std::process::id()));
        fs::create_dir_all(&root).expect("create search directory");
        let path = root.join("long.txt");
        fs::write(&path, "a".repeat(10_000)).expect("write long line");
        let options = FileSearchOptions {
            pattern: "a+".to_string(),
            case_sensitive: true,
            whole_word: false,
            is_regex: true,
            max_results: 10,
        };

        let results = search_paths(
            SearchScope::Workspace(&root),
            std::iter::once(path),
            &options,
            || true,
        )
        .expect("search should succeed");
        assert_eq!(results[0].1[0].end, 10_000);
        assert_eq!(results[0].1[0].line_content.len(), 200);
        assert_eq!(results[0].1[0].preview_match, 0..200);
        fs::remove_dir_all(root).expect("remove search directory");
    }

    #[cfg(unix)]
    #[test]
    fn does_not_search_symlinks_outside_the_workspace() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir()
            .join(format!("ahead-search-symlink-{}", std::process::id()));
        let workspace = root.join("workspace");
        fs::create_dir_all(&workspace).expect("create workspace");
        let outside = root.join("private.txt");
        fs::write(&outside, "private content\n").expect("write outside file");
        symlink(&outside, workspace.join("link.txt")).expect("link outside file");

        assert!(workspace_files(&workspace).is_empty());
        fs::remove_dir_all(root).expect("remove search directory");
    }
}
