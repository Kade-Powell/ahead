use std::{
    borrow::Cow,
    fs,
    fs::File,
    io::{Read, Write},
    ops::RangeBounds,
    path::{Path, PathBuf},
    time::SystemTime,
};

use ahead_core::directory::Directory;
use ahead_core::encoding::offset_utf8_to_utf16;
use ahead_rpc::{buffer::BufferId, delta::AheadDelta};
use anyhow::{Result, anyhow};
use lsp_types::*;
use ropey::{LineType, Rope};

#[derive(Clone)]
pub struct Buffer {
    pub language_id: String,
    pub read_only: bool,
    pub id: BufferId,
    pub rope: Rope,
    pub path: PathBuf,
    pub rev: u64,
    pub mod_time: Option<SystemTime>,
}

impl Buffer {
    pub fn new(id: BufferId, path: PathBuf) -> Buffer {
        let (s, read_only) = match load_file(&path) {
            Ok(s) => (s, false),
            Err(err) => {
                use std::io::ErrorKind;
                match err.downcast_ref::<std::io::Error>() {
                    Some(err) => match err.kind() {
                        ErrorKind::PermissionDenied => {
                            ("Permission Denied".to_string(), true)
                        }
                        ErrorKind::NotFound => ("".to_string(), false),
                        ErrorKind::OutOfMemory => {
                            ("File too big (out of memory)".to_string(), false)
                        }
                        _ => (format!("Not supported: {err}"), true),
                    },
                    None => (format!("Not supported: {err}"), true),
                }
            }
        };
        let language_id =
            language_id_from_path_with_content(&path, Some(&s)).unwrap_or_default();
        let rope = Rope::from(s);
        let rev = u64::from(rope.len() != 0);
        let mod_time = get_mod_time(&path);
        Buffer {
            id,
            rope,
            read_only,
            path,
            language_id,
            rev,
            mod_time,
        }
    }

    pub fn save(&mut self, rev: u64, create_parents: bool) -> Result<()> {
        if self.read_only {
            return Err(anyhow!("can't save to read only file"));
        }

        if self.rev != rev {
            return Err(anyhow!("not the right rev"));
        }
        let path = if self.path.is_symlink() {
            self.path.canonicalize()?
        } else {
            self.path.clone()
        };
        let parent = path.parent().ok_or_else(|| anyhow!("file has no parent"))?;
        if create_parents {
            fs::create_dir_all(parent)?;
        }
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        match fs::metadata(&path) {
            Ok(metadata) => {
                if metadata.permissions().readonly() {
                    return Err(anyhow!("can't save to read only file"));
                }
                file.as_file().set_permissions(metadata.permissions())?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        for chunk in self.rope.chunks() {
            file.write_all(chunk.as_bytes())?;
        }
        file.as_file().sync_all()?;
        file.persist(&path)?;
        self.mod_time = get_mod_time(&path);
        Ok(())
    }

    pub fn update(
        &mut self,
        delta: &AheadDelta,
        rev: u64,
    ) -> Option<TextDocumentContentChangeEvent> {
        if self.rev + 1 != rev {
            return None;
        }
        self.rev += 1;
        let content_change = get_document_content_changes(delta, self);
        self.rope = delta.apply(&self.rope);
        Some(
            content_change.unwrap_or_else(|| TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: self.get_document(),
            }),
        )
    }

    pub fn get_document(&self) -> String {
        self.rope.to_string()
    }

    pub fn offset_of_line(&self, line: usize) -> usize {
        self.rope.line_to_byte_idx(line, LineType::LF_CR)
    }

    pub fn line_of_offset(&self, offset: usize) -> usize {
        self.rope.byte_to_line_idx(offset, LineType::LF_CR)
    }

    pub fn offset_to_line_col(&self, offset: usize) -> (usize, usize) {
        let line = self.line_of_offset(offset);
        (line, offset - self.offset_of_line(line))
    }

    /// Converts a UTF8 offset to a UTF16 LSP position  
    pub fn offset_to_position(&self, offset: usize) -> Position {
        let (line, col) = self.offset_to_line_col(offset);
        // Get the offset of line to make the conversion cheaper, rather than working
        // from the very start of the document to `offset`
        let line_offset = self.offset_of_line(line);
        let utf16_col =
            offset_utf8_to_utf16(self.char_indices_iter(line_offset..), col);

        Position {
            line: line as u32,
            character: utf16_col as u32,
        }
    }

    pub fn slice_to_cow<R: RangeBounds<usize>>(&self, range: R) -> Cow<'_, str> {
        Cow::from(self.rope.slice(range))
    }

    pub fn line_to_cow(&self, line: usize) -> Cow<'_, str> {
        Cow::from(
            self.rope
                .slice(self.offset_of_line(line)..self.offset_of_line(line + 1)),
        )
    }

    /// Iterate over (utf8_offset, char) values in the given range.
    /// Offsets are relative to the start of `range`, matching the previous
    /// chunk-joined behavior; see the `char_indices_are_slice_relative`
    /// test that pins this.
    pub fn char_indices_iter<R: RangeBounds<usize>>(
        &self,
        range: R,
    ) -> impl Iterator<Item = (usize, char)> + '_ {
        self.rope.slice(range).char_indices()
    }

    pub fn len(&self) -> usize {
        self.rope.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

pub fn load_file(path: &Path) -> Result<String> {
    read_path_to_string(path)
}

pub fn read_path_to_string<P: AsRef<Path>>(path: P) -> Result<String> {
    let path = path.as_ref();

    let mut file = File::open(path)?;
    // Read the file in as bytes
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;

    // Parse the file contents as utf8
    let contents = String::from_utf8(buffer)?;

    Ok(contents.to_string())
}

pub fn language_id_from_path(path: &Path) -> Option<String> {
    language_id_from_path_with_content(path, None)
}

pub fn language_id_from_path_with_content(
    path: &Path,
    content: Option<&str>,
) -> Option<String> {
    let root = Directory::plugins_directory();
    language_id_with_extensions(path, content, root.as_deref())
}

fn language_id_with_extensions(
    path: &Path,
    content: Option<&str>,
    extensions_root: Option<&Path>,
) -> Option<String> {
    if let Some(root) = extensions_root {
        match ahead_extension_host::language_id_for_path(root, path, content) {
            Ok(Some(language)) => return Some(language),
            Ok(None) => {}
            Err(error) => tracing::warn!(?error, "resolving extension language"),
        }
    }
    builtin_language_id(path)
}

fn builtin_language_id(path: &Path) -> Option<String> {
    // recommended language_id values
    // https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocumentItem
    Some(match path.extension() {
        Some(ext) => {
            match ext.to_str()? {
                "C" | "H" => "cpp",
                "M" => "objective-c",
                // stop case-sensitive matching
                ext => match ext.to_lowercase().as_str() {
                    "bat" => "bat",
                    "clj" | "cljs" | "cljc" | "edn" => "clojure",
                    "coffee" => "coffeescript",
                    "c" | "h" => "c",
                    "cpp" | "hpp" | "cxx" | "hxx" | "c++" | "h++" | "cc" | "hh" => {
                        "cpp"
                    }
                    "cs" | "csx" => "csharp",
                    "css" => "css",
                    "d" | "di" | "dlang" => "dlang",
                    "diff" | "patch" => "diff",
                    "dart" => "dart",
                    "dockerfile" => "dockerfile",
                    "elm" => "elm",
                    "ex" | "exs" => "elixir",
                    "erl" | "hrl" => "erlang",
                    "fs" | "fsi" | "fsx" | "fsscript" => "fsharp",
                    "git-commit" | "git-rebase" => "git",
                    "go" => "go",
                    "groovy" | "gvy" | "gy" | "gsh" => "groovy",
                    "hbs" => "handlebars",
                    "htm" | "html" | "xhtml" => "html",
                    "ini" => "ini",
                    "java" | "class" => "java",
                    "js" | "mjs" | "cjs" => "javascript",
                    "jsx" => "javascriptreact",
                    "json" => "json",
                    "jl" => "julia",
                    "kt" => "kotlin",
                    "kts" => "kotlinbuildscript",
                    "less" => "less",
                    "lua" => "lua",
                    "makefile" | "gnumakefile" => "makefile",
                    "md" | "markdown" => "markdown",
                    "m" => "objective-c",
                    "mm" => "objective-cpp",
                    "plx" | "pl" | "pm" | "xs" | "t" | "pod" | "cgi" => "perl",
                    "p6" | "pm6" | "pod6" | "t6" | "raku" | "rakumod"
                    | "rakudoc" | "rakutest" => "perl6",
                    "php" | "phtml" | "pht" | "phps" => "php",
                    "proto" => "proto",
                    "ps1" | "ps1xml" | "psc1" | "psm1" | "psd1" | "pssc"
                    | "psrc" => "powershell",
                    "py" | "pyi" | "pyw" => "python",
                    "r" => "r",
                    "rb" => "ruby",
                    "rs" => "rust",
                    "scss" | "sass" => "scss",
                    "sc" | "scala" => "scala",
                    "sh" | "bash" | "zsh" => "shellscript",
                    "sql" => "sql",
                    "swift" => "swift",
                    "svelte" => "svelte",
                    "thrift" => "thrift",
                    "toml" => "toml",
                    "ts" | "mts" | "cts" => "typescript",
                    "tsx" => "typescriptreact",
                    "tex" => "tex",
                    "vb" => "vb",
                    "xml" | "csproj" => "xml",
                    "xsl" => "xsl",
                    "yml" | "yaml" => "yaml",
                    "zig" => "zig",
                    "vue" => "vue",
                    _ => return None,
                },
            }
        }
        // Handle paths without extension
        #[allow(clippy::match_single_binding)]
        None => match path.file_name()?.to_str()? {
            // case-insensitive matching
            filename => match filename.to_lowercase().as_str() {
                "dockerfile" => "dockerfile",
                "makefile" | "gnumakefile" => "makefile",
                _ => return None,
            },
        },
    })
    .map(str::to_string)
}

fn get_document_content_changes(
    delta: &AheadDelta,
    buffer: &Buffer,
) -> Option<TextDocumentContentChangeEvent> {
    let (start, end) = delta.summary();

    // TODO: Handle more trivial cases like typing when there's a selection or transpose
    if let Some(node) = delta.as_simple_insert() {
        let start = buffer.offset_to_position(start);

        let end = buffer.offset_to_position(end);

        Some(TextDocumentContentChangeEvent {
            range: Some(Range { start, end }),
            range_length: None,
            text: node.to_string(),
        })
    }
    // Or a simple delete
    else if delta.is_simple_delete() {
        let end_position = buffer.offset_to_position(end);

        let start = buffer.offset_to_position(start);

        Some(TextDocumentContentChangeEvent {
            range: Some(Range {
                start,
                end: end_position,
            }),
            range_length: None,
            text: String::new(),
        })
    } else {
        None
    }
}

/// Returns the modification timestamp for the file at a given path,
/// if present.
pub fn get_mod_time<P: AsRef<Path>>(path: P) -> Option<SystemTime> {
    File::open(path)
        .and_then(|f| f.metadata())
        .and_then(|meta| meta.modified())
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_detection_reaches_extensions_and_keeps_builtin_languages() {
        let root = tempfile::tempdir().expect("extension root");
        let language = root.path().join("gleam/languages/gleam");
        std::fs::create_dir_all(&language).expect("language directory");
        std::fs::write(
            root.path().join("gleam/extension.toml"),
            "id = 'gleam'\nlanguages = ['languages/gleam']\n",
        )
        .expect("manifest");
        std::fs::write(language.join("config.toml"), "name = 'Gleam'\npath_suffixes = ['gleam', 'special.ts']\nfirst_line_pattern = '^#!.*gleam'\n").expect("language config");
        for (path, expected) in [
            ("main.gleam", Some("Gleam")),
            ("main.special.ts", Some("Gleam")),
            ("main.rs", Some("rust")),
            ("main.py", Some("python")),
            ("main.ts", Some("typescript")),
            ("main.mts", Some("typescript")),
            ("main.cts", Some("typescript")),
            ("main.tsx", Some("typescriptreact")),
            ("main.mjs", Some("javascript")),
            ("main.cjs", Some("javascript")),
            ("main.jsx", Some("javascriptreact")),
            ("main.pyc", None),
        ] {
            assert_eq!(
                language_id_with_extensions(
                    Path::new(path),
                    None,
                    Some(root.path())
                )
                .as_deref(),
                expected,
                "{path}"
            );
        }
        assert_eq!(
            language_id_with_extensions(
                Path::new("script"),
                Some("#!/usr/bin/gleam\n"),
                Some(root.path())
            )
            .as_deref(),
            Some("Gleam")
        );
        assert_eq!(
            language_id_with_extensions(
                Path::new("script"),
                Some("not a shebang\n#!/usr/bin/gleam\n"),
                Some(root.path())
            ),
            None
        );
    }
    use ahead_rpc::delta::DeltaOp;

    #[test]
    fn save_preserves_existing_backup_and_rejects_stale_revisions() {
        let directory = tempfile::tempdir().expect("test directory");
        let path = directory.path().join("main.rs");
        let backup = path.with_extension("rs.bak");
        fs::write(&path, "original").expect("source");
        fs::write(&backup, "user backup").expect("existing backup");
        let mut buffer = Buffer::new(BufferId::next(), path.clone());
        buffer.rope = Rope::from("new contents 🦀\n");
        assert!(buffer.save(buffer.rev + 1, false).is_err());
        assert_eq!(fs::read_to_string(&path).expect("source"), "original");
        buffer.save(buffer.rev, false).expect("save buffer");
        assert_eq!(
            fs::read_to_string(&path).expect("saved source"),
            "new contents 🦀\n"
        );
        assert_eq!(
            fs::read_to_string(&backup).expect("existing backup"),
            "user backup"
        );
    }

    #[cfg(unix)]
    #[test]
    fn save_preserves_symlink_and_executable_permissions() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let directory = tempfile::tempdir().expect("test directory");
        let target = directory.path().join("script.sh");
        let path = directory.path().join("linked.sh");
        fs::write(&target, "old").expect("source");
        fs::set_permissions(&target, fs::Permissions::from_mode(0o755))
            .expect("executable source");
        symlink(&target, &path).expect("source link");
        let mut buffer = Buffer::new(BufferId::next(), path.clone());
        buffer.rope = Rope::from("new");
        buffer.save(buffer.rev, false).expect("save through link");
        assert!(path.is_symlink());
        assert_eq!(fs::read_to_string(&target).expect("target"), "new");
        assert_eq!(
            fs::metadata(&target)
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
    }

    fn buffer_with(name: &str, text: &str) -> Buffer {
        let path = std::env::temp_dir().join(format!(
            "ahead-buffer-test-{}-{name}.txt",
            std::process::id()
        ));
        std::fs::write(&path, text).unwrap();
        Buffer::new(BufferId::next(), path)
    }

    #[test]
    fn line_offsets_round_trip() {
        let buffer = buffer_with("offsets", "ab\ncde\nf");
        assert_eq!(buffer.offset_of_line(1), 3);
        assert_eq!(buffer.line_of_offset(5), 1);
        assert_eq!(buffer.line_to_cow(1).as_ref(), "cde\n");
    }

    #[test]
    fn char_indices_are_slice_relative() {
        let buffer = buffer_with("charidx", "ab\ncde\nf");
        let collected: Vec<(usize, char)> = buffer.char_indices_iter(3..).collect();
        assert_eq!(
            collected,
            vec![('c', 0), ('d', 1), ('e', 2), ('\n', 3), ('f', 4)]
                .into_iter()
                .map(|(ch, off)| (off, ch))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn update_applies_delta_and_bumps_rev() {
        let mut buffer = buffer_with("update", "hello world");
        assert_eq!(buffer.rev, 1);
        let delta = AheadDelta::new(
            buffer.len(),
            vec![
                DeltaOp::Retain(6),
                DeltaOp::Insert("wonderful ".to_string()),
                DeltaOp::Retain(5),
            ],
        );
        let change = buffer.update(&delta, 2).unwrap();
        assert_eq!(buffer.get_document(), "hello wonderful world");
        assert_eq!(buffer.rev, 2);
        let range = change.range.unwrap();
        assert_eq!((range.start.line, range.start.character), (0, 6));
        assert_eq!((range.end.line, range.end.character), (0, 6));
        assert_eq!(change.text, "wonderful ");
    }

    #[test]
    fn update_rejects_wrong_rev() {
        let mut buffer = buffer_with("reject", "hello");
        let delta =
            AheadDelta::new(buffer.len(), vec![DeltaOp::Insert("!".to_string())]);
        assert!(buffer.update(&delta, 99).is_none());
        assert_eq!(buffer.get_document(), "hello");
    }
}
