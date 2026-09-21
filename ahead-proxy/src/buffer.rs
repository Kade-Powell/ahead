use std::{
    borrow::Cow,
    ffi::OsString,
    fs,
    fs::File,
    io::{Read, Write},
    ops::RangeBounds,
    path::{Path, PathBuf},
    time::SystemTime,
};

use anyhow::{Result, anyhow};
use ahead_core::encoding::offset_utf8_to_utf16;
use ahead_rpc::{buffer::BufferId, delta::AheadDelta};
use lsp_types::*;
use ropey::{LineType, Rope};

#[derive(Clone)]
pub struct Buffer {
    pub language_id: &'static str,
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
        let rope = Rope::from(s);
        let rev = u64::from(rope.len() != 0);
        let language_id = language_id_from_path(&path).unwrap_or("");
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
        let bak_extension = self.path.extension().map_or_else(
            || OsString::from("bak"),
            |ext| {
                let mut ext = ext.to_os_string();
                ext.push(".bak");
                ext
            },
        );
        let path = if self.path.is_symlink() {
            self.path.canonicalize()?
        } else {
            self.path.clone()
        };
        let new_file = !path.exists();

        let bak_file_path = &path.with_extension(bak_extension);
        if !new_file {
            fs::copy(&path, bak_file_path)?;
        }

        if create_parents {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
        }

        let mut f = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&path)?;
        for chunk in self.rope.chunks() {
            f.write_all(chunk.as_bytes())?;
        }

        self.mod_time = get_mod_time(&path);
        if !new_file {
            fs::remove_file(bak_file_path)?;
        }

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

pub fn language_id_from_path(path: &Path) -> Option<&'static str> {
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
                    "js" => "javascript",
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
                    "py" | "pyi" | "pyc" | "pyd" | "pyw" => "python",
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
                    "ts" => "typescript",
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
    use ahead_rpc::delta::DeltaOp;

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
        let delta = AheadDelta::new(
            buffer.len(),
            vec![DeltaOp::Insert("!".to_string())],
        );
        assert!(buffer.update(&delta, 99).is_none());
        assert_eq!(buffer.get_document(), "hello");
    }
}
