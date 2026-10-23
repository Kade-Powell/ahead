use std::{collections::BTreeMap, ops::Range, path::Path};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Snippet {
    pub(crate) text: String,
    pub(crate) tabstops: Vec<Tabstop>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Tabstop {
    pub(crate) index: u32,
    pub(crate) ranges: Vec<Range<usize>>,
    pub(crate) choices: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Variables(BTreeMap<String, String>);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct CommentVariables {
    pub(crate) line_comment: Option<String>,
    pub(crate) block_comment: Option<(String, String)>,
}

impl Variables {
    pub(crate) fn for_completion(
        text: &str,
        cursor: usize,
        line_index: usize,
        path: &Path,
        workspace: &Path,
        selected_text: &str,
        clipboard: &str,
        extension_comments: Option<&CommentVariables>,
    ) -> Self {
        let mut values = BTreeMap::new();
        let lines = text.split_terminator('\n').collect::<Vec<_>>();
        let current_line = lines.get(line_index).copied().unwrap_or_default();
        let current_line = current_line.strip_suffix('\r').unwrap_or(current_line);
        let line_start = text
            .split_inclusive('\n')
            .take(line_index)
            .map(str::len)
            .sum::<usize>();
        let line_cursor = cursor.saturating_sub(line_start).min(current_line.len());
        let word_start = current_line[..line_cursor]
            .char_indices()
            .rev()
            .take_while(|(_, character)| is_word_character(*character))
            .last()
            .map_or(line_cursor, |(index, _)| index);
        let word_end = current_line[line_cursor..]
            .char_indices()
            .take_while(|(_, character)| is_word_character(*character))
            .last()
            .map_or(line_cursor, |(index, character)| {
                line_cursor + index + character.len_utf8()
            });

        values.insert("TM_SELECTED_TEXT".into(), selected_text.into());
        values.insert("TM_CURRENT_LINE".into(), current_line.into());
        values.insert(
            "TM_CURRENT_WORD".into(),
            current_line[word_start..word_end].into(),
        );
        values.insert("TM_LINE_INDEX".into(), line_index.to_string());
        values.insert("TM_LINE_NUMBER".into(), (line_index + 1).to_string());
        values.insert("CURSOR_INDEX".into(), "0".into());
        values.insert("CURSOR_NUMBER".into(), "1".into());
        values.insert("CLIPBOARD".into(), clipboard.into());
        values.insert("TM_FILENAME".into(), String::new());
        values.insert("TM_FILENAME_BASE".into(), String::new());
        values.insert("TM_DIRECTORY".into(), String::new());
        values.insert("TM_FILEPATH".into(), String::new());
        values.insert("RELATIVE_FILEPATH".into(), String::new());
        values.insert("WORKSPACE_NAME".into(), String::new());
        values.insert(
            "WORKSPACE_FOLDER".into(),
            workspace.to_string_lossy().into_owned(),
        );
        let mut comments = comment_variables(path);
        if let Some(extension_comments) = extension_comments {
            if extension_comments.line_comment.is_some() {
                comments.line_comment = extension_comments.line_comment.clone();
            }
            if extension_comments.block_comment.is_some() {
                comments.block_comment = extension_comments.block_comment.clone();
            }
        }
        if let Some(line_comment) = comments.line_comment {
            values.insert("LINE_COMMENT".into(), line_comment);
        }
        if let Some((start, end)) = comments.block_comment {
            values.insert("BLOCK_COMMENT_START".into(), start);
            values.insert("BLOCK_COMMENT_END".into(), end);
        }

        if let Some(filename) = path.file_name() {
            values.insert(
                "TM_FILENAME".into(),
                filename.to_string_lossy().into_owned(),
            );
        }
        if let Some(filename_base) = path.file_stem() {
            values.insert(
                "TM_FILENAME_BASE".into(),
                filename_base.to_string_lossy().into_owned(),
            );
        }
        if let Some(directory) = path.parent() {
            values.insert(
                "TM_DIRECTORY".into(),
                directory.to_string_lossy().into_owned(),
            );
        }
        values.insert("TM_FILEPATH".into(), path.to_string_lossy().into_owned());
        if !workspace.as_os_str().is_empty()
            && let Ok(relative_path) = path.strip_prefix(workspace)
        {
            values.insert(
                "RELATIVE_FILEPATH".into(),
                relative_path.to_string_lossy().into_owned(),
            );
        }
        if let Some(workspace_name) = workspace.file_name() {
            values.insert(
                "WORKSPACE_NAME".into(),
                workspace_name.to_string_lossy().into_owned(),
            );
        }

        let now = chrono::Local::now();
        values.extend([
            ("CURRENT_YEAR".into(), now.format("%Y").to_string()),
            ("CURRENT_YEAR_SHORT".into(), now.format("%y").to_string()),
            ("CURRENT_MONTH".into(), now.format("%m").to_string()),
            ("CURRENT_MONTH_NAME".into(), now.format("%B").to_string()),
            (
                "CURRENT_MONTH_NAME_SHORT".into(),
                now.format("%b").to_string(),
            ),
            ("CURRENT_DATE".into(), now.format("%d").to_string()),
            ("CURRENT_DAY_NAME".into(), now.format("%A").to_string()),
            (
                "CURRENT_DAY_NAME_SHORT".into(),
                now.format("%a").to_string(),
            ),
            ("CURRENT_HOUR".into(), now.format("%H").to_string()),
            ("CURRENT_MINUTE".into(), now.format("%M").to_string()),
            ("CURRENT_SECOND".into(), now.format("%S").to_string()),
            ("CURRENT_MILLISECOND".into(), now.format("%3f").to_string()),
            ("CURRENT_SECONDS_UNIX".into(), now.timestamp().to_string()),
            (
                "CURRENT_MILLISECONDS_UNIX".into(),
                now.timestamp_millis().to_string(),
            ),
            (
                "CURRENT_TIMEZONE_OFFSET".into(),
                now.format("%:z").to_string(),
            ),
        ]);
        let timezone_name = match iana_time_zone::get_timezone() {
            Ok(timezone_name) => timezone_name,
            Err(error) => {
                log::debug!(
                    "Could not resolve local IANA timezone for snippet: {error}"
                );
                String::new()
            }
        };
        values.insert("CURRENT_TIMEZONE_NAME".into(), timezone_name);
        let random_seed = uuid::Uuid::new_v4().as_u128();
        values.insert("RANDOM".into(), format!("{:06}", random_seed % 1_000_000));
        values.insert(
            "RANDOM_HEX".into(),
            format!("{:06x}", random_seed & 0x00ff_ffff),
        );
        values.insert("UUID".into(), uuid::Uuid::new_v4().to_string());

        Self(values)
    }
}

fn comment_variables(path: &Path) -> CommentVariables {
    let Some(extension) = path.extension().and_then(|extension| extension.to_str())
    else {
        return CommentVariables::default();
    };
    match extension.to_ascii_lowercase().as_str() {
        "rs" | "c" | "h" | "cc" | "cpp" | "hpp" | "cs" | "java" | "kt" | "kts"
        | "go" | "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "mts" | "cts"
        | "swift" => CommentVariables {
            line_comment: Some("//".into()),
            block_comment: Some(("/*".into(), "*/".into())),
        },
        "py" | "pyi" => CommentVariables {
            line_comment: Some("#".into()),
            block_comment: None,
        },
        "html" | "htm" | "xml" | "svg" => CommentVariables {
            line_comment: None,
            block_comment: Some(("<!--".into(), "-->".into())),
        },
        _ => CommentVariables::default(),
    }
}

fn is_word_character(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

#[derive(Clone, Debug)]
pub(crate) struct Session {
    text: String,
    tabstops: Vec<Tabstop>,
    active: usize,
    active_choice: usize,
}

impl Snippet {
    #[cfg(test)]
    pub(crate) fn parse(source: &str) -> Result<Self, String> {
        Self::parse_with_variables(source, &Variables::default())
    }

    pub(crate) fn parse_with_variables(
        source: &str,
        variables: &Variables,
    ) -> Result<Self, String> {
        let mut parser = Parser {
            source,
            offset: 0,
            text: String::with_capacity(source.len()),
            tabstops: BTreeMap::new(),
            choices: BTreeMap::new(),
            tabstop_defaults: BTreeMap::new(),
            unresolved_variables: Vec::new(),
            variables,
        };
        parser.parse_sequence(false)?;

        let mut variable_indices = BTreeMap::new();
        let mut next_variable_index = parser
            .tabstops
            .keys()
            .filter(|index| **index != 0)
            .max()
            .copied()
            .unwrap_or(0)
            .checked_add(1);
        for (name, range) in std::mem::take(&mut parser.unresolved_variables) {
            let index = if let Some(index) = variable_indices.get(&name) {
                *index
            } else {
                let index = next_variable_index
                    .ok_or("The server returned too many snippet placeholders")?;
                next_variable_index = index.checked_add(1);
                variable_indices.insert(name, index);
                index
            };
            parser
                .tabstops
                .entry(index)
                .or_insert_with(|| Tabstop {
                    index,
                    ranges: Vec::new(),
                    choices: Vec::new(),
                })
                .ranges
                .push(range);
        }
        parser.expand_forward_tabstop_references()?;

        let final_tabstop = parser.tabstops.remove(&0);
        let mut tabstops = parser.tabstops.into_values().collect::<Vec<_>>();
        if let Some(final_tabstop) = final_tabstop {
            tabstops.push(final_tabstop);
        } else if !tabstops.last().is_some_and(|tabstop| {
            tabstop.ranges.first() == Some(&(parser.text.len()..parser.text.len()))
        }) {
            tabstops.push(Tabstop {
                index: 0,
                ranges: vec![parser.text.len()..parser.text.len()],
                choices: Vec::new(),
            });
        }

        Ok(Self {
            text: parser.text,
            tabstops,
        })
    }
}

impl Session {
    pub(crate) fn new(text: String, tabstops: Vec<Tabstop>) -> Option<Self> {
        (tabstops.first()?.index != 0).then_some(Self {
            text,
            tabstops,
            active: 0,
            active_choice: 0,
        })
    }

    pub(crate) fn active_range(&self) -> Option<Range<usize>> {
        self.tabstops.get(self.active)?.ranges.first().cloned()
    }

    pub(crate) fn active_contains(&self, selection: &Range<usize>) -> bool {
        self.tabstops.get(self.active).is_some_and(|tabstop| {
            tabstop.ranges.iter().any(|range| {
                range.start <= selection.start
                    && selection.end <= range.end
                    && selection.start <= selection.end
            })
        })
    }

    pub(crate) fn active_index(&self) -> usize {
        self.active
    }

    pub(crate) fn active_is_final(&self) -> bool {
        self.tabstops
            .get(self.active)
            .is_some_and(|tabstop| tabstop.index == 0)
    }

    pub(crate) fn active_choices(&self) -> &[String] {
        let Some(tabstop) = self.tabstops.get(self.active) else {
            return &[];
        };
        if tabstop.ranges.len() != 1 {
            return &[];
        }
        let Some(range) = tabstop.ranges.first() else {
            return &[];
        };
        let Some(choice) = tabstop.choices.get(self.active_choice) else {
            return &[];
        };
        if self.text.get(range.clone()) == Some(choice.as_str()) {
            &tabstop.choices
        } else {
            &[]
        }
    }

    pub(crate) fn active_choice_index(&self) -> usize {
        self.active_choice
    }

    pub(crate) fn cycle_choice(
        &mut self,
        forward: bool,
    ) -> Option<(Range<usize>, Range<usize>, String)> {
        let tabstop = self.tabstops.get(self.active)?;
        if tabstop.ranges.len() != 1 || tabstop.choices.len() < 2 {
            return None;
        }
        let range = tabstop.ranges.first()?.clone();
        let choice_index = if forward {
            (self.active_choice + 1) % tabstop.choices.len()
        } else if self.active_choice == 0 {
            tabstop.choices.len() - 1
        } else {
            self.active_choice - 1
        };
        let replacement = tabstop.choices.get(choice_index)?.clone();
        let delta = replacement.len() as isize - range.len() as isize;
        let mut updated_tabstops = self.tabstops.clone();
        for stop in &mut updated_tabstops {
            for stop_range in &mut stop.ranges {
                if stop_range == &range {
                    *stop_range = range.start..range.start + replacement.len();
                } else if stop_range.end <= range.start {
                    continue;
                } else if stop_range.start >= range.end {
                    stop_range.start = stop_range.start.checked_add_signed(delta)?;
                    stop_range.end = stop_range.end.checked_add_signed(delta)?;
                } else {
                    return None;
                }
            }
        }

        self.text.replace_range(range.clone(), &replacement);
        self.tabstops = updated_tabstops;
        self.active_choice = choice_index;
        let next_range = range.start..range.start + replacement.len();
        Some((range, next_range, replacement))
    }

    pub(crate) fn move_to(&mut self, next: usize) -> Option<Range<usize>> {
        self.active = next;
        self.active_choice = 0;
        self.active_range()
    }

    pub(crate) fn can_move_to(&self, next: usize) -> bool {
        next < self.tabstops.len()
    }

    pub(crate) fn update_after_edit(
        &mut self,
        new_text: String,
        selection: Range<usize>,
    ) -> bool {
        let Some((old_edit, new_edit)) = text_diff(&self.text, &new_text) else {
            return false;
        };
        let Some(active_index) =
            self.tabstops.get(self.active).map(|tabstop| tabstop.index)
        else {
            return false;
        };
        if !self.tabstops.get(self.active).is_some_and(|tabstop| {
            tabstop
                .ranges
                .iter()
                .any(|range| contains_edit(range, &old_edit))
        }) {
            return false;
        }

        let delta = new_edit.len() as isize - old_edit.len() as isize;
        for tabstop in &mut self.tabstops {
            for range in &mut tabstop.ranges {
                if contains_edit(range, &old_edit)
                    && (tabstop.index == active_index
                        || range.start < old_edit.start
                        || old_edit.end < range.end)
                {
                    let Some(end) = range.end.checked_add_signed(delta) else {
                        return false;
                    };
                    *range = range.start..end;
                    continue;
                }
                if range.end <= old_edit.start {
                    continue;
                }
                if range.start >= old_edit.end {
                    let Some(start) = range.start.checked_add_signed(delta) else {
                        return false;
                    };
                    let Some(end) = range.end.checked_add_signed(delta) else {
                        return false;
                    };
                    *range = start..end;
                    continue;
                }
                return false;
            }
        }

        self.text = new_text;
        if let Some(tabstop) = self.tabstops.get(self.active)
            && let Some(range) = tabstop.ranges.first()
            && let Some(current_text) = self.text.get(range.clone())
            && let Some(choice_index) = tabstop
                .choices
                .iter()
                .position(|choice| choice == current_text)
        {
            self.active_choice = choice_index;
        }
        self.active_range().is_some_and(|range| {
            range.start <= selection.start
                && selection.end <= range.end
                && selection.start <= selection.end
        })
    }
}

struct Parser<'a> {
    source: &'a str,
    offset: usize,
    text: String,
    tabstops: BTreeMap<u32, Tabstop>,
    choices: BTreeMap<u32, Vec<String>>,
    tabstop_defaults: BTreeMap<u32, String>,
    unresolved_variables: Vec<(String, Range<usize>)>,
    variables: &'a Variables,
}

impl Parser<'_> {
    fn parse_sequence(&mut self, nested: bool) -> Result<(), String> {
        while let Some(character) = self.peek() {
            match character {
                '$' => self.parse_tabstop()?,
                '\\' => self.parse_escape(),
                '}' if nested => {
                    self.offset += 1;
                    return Ok(());
                }
                character => {
                    self.text.push(character);
                    self.offset += character.len_utf8();
                }
            }
        }

        if nested {
            Err("The server returned an unclosed snippet placeholder".into())
        } else {
            Ok(())
        }
    }

    fn parse_tabstop(&mut self) -> Result<(), String> {
        self.offset += 1;
        let braced = self.consume('{');
        let Some(index) = self.parse_index()? else {
            if self.peek().is_some_and(is_variable_start) {
                return self.parse_variable(braced);
            }
            if braced {
                return Err("The server returned an invalid snippet variable".into());
            }
            self.text.push('$');
            return Ok(());
        };

        let Some(next) = self.peek() else {
            if braced {
                return Err("The server returned an unclosed snippet tabstop".into());
            }
            self.add_tabstop_or_default(index);
            return Ok(());
        };

        if braced && next == ':' {
            self.offset += 1;
            let start = self.text.len();
            self.parse_sequence(true)?;
            let range = start..self.text.len();
            let default = self.text[range.clone()].to_owned();
            self.tabstop_defaults.entry(index).or_insert(default);
            self.add_tabstop(index, range);
        } else if braced && next == '|' {
            self.offset += 1;
            let start = self.text.len();
            let choices = self.parse_choice()?;
            let range = start..self.text.len();
            let default = self.text[range.clone()].to_owned();
            self.tabstop_defaults.entry(index).or_insert(default);
            if !choices.is_empty() {
                self.choices.entry(index).or_insert(choices);
            }
            self.add_tabstop(index, range);
        } else if braced && next == '}' {
            self.offset += 1;
            self.add_tabstop_or_default(index);
        } else if braced {
            return Err("The server returned an invalid snippet tabstop".into());
        } else {
            self.add_tabstop_or_default(index);
        }

        Ok(())
    }

    fn parse_variable(&mut self, braced: bool) -> Result<(), String> {
        let start = self.offset;
        self.offset += self.peek().map_or(0, char::len_utf8);
        while self.peek().is_some_and(is_variable_character) {
            self.offset += self.peek().map_or(0, char::len_utf8);
        }
        let name = self.source[start..self.offset].to_owned();
        let value = self.variables.0.get(&name).cloned();

        if !braced {
            if let Some(value) = value {
                self.text.push_str(&value);
            } else {
                self.append_unknown_variable(&name, &name);
            }
            return Ok(());
        }

        match self.peek() {
            Some('}') => {
                self.offset += 1;
                if let Some(value) = value {
                    self.text.push_str(&value);
                } else {
                    self.append_unknown_variable(&name, &name);
                }
                Ok(())
            }
            Some(':') => {
                self.offset += 1;
                if let Some(value) = value {
                    self.skip_sequence()?;
                    self.text.push_str(&value);
                } else {
                    let start = self.text.len();
                    self.parse_sequence(true)?;
                    self.unresolved_variables
                        .push((name, start..self.text.len()));
                }
                Ok(())
            }
            Some('/') => {
                self.offset += 1;
                let pattern = self.parse_transform_segment(false)?;
                let format = self.parse_transform_segment(true)?;
                let mut options = String::new();
                while let Some(character) = self.peek() {
                    if character == '}' {
                        break;
                    }
                    options.push(character);
                    self.offset += character.len_utf8();
                }
                if !self.consume('}') {
                    return Err(
                        "The server returned an unclosed snippet transform".into()
                    );
                }

                let input = value.as_deref().unwrap_or(&name);
                let transformed =
                    apply_transform(input, &pattern, &format, &options)?;
                if value.is_some() {
                    self.text.push_str(&transformed);
                } else {
                    self.append_unknown_variable(&name, &transformed);
                }
                Ok(())
            }
            Some(_) => Err("The server returned an invalid snippet variable".into()),
            None => Err("The server returned an unclosed snippet variable".into()),
        }
    }

    fn parse_transform_segment(
        &mut self,
        format_segment: bool,
    ) -> Result<String, String> {
        let mut segment = String::new();
        let mut nested_format = 0usize;
        while let Some(character) = self.peek() {
            if character == '\\' {
                self.offset += 1;
                match self.peek() {
                    Some('/') => {
                        self.offset += 1;
                        segment.push('/');
                    }
                    Some(escaped @ ('$' | '}' | '\\')) if format_segment => {
                        self.offset += escaped.len_utf8();
                        segment.push('\\');
                        segment.push(escaped);
                    }
                    _ => segment.push('\\'),
                }
                continue;
            }
            if format_segment && self.source[self.offset..].starts_with("${") {
                self.offset += 2;
                nested_format = nested_format.saturating_add(1);
                segment.push_str("${");
                continue;
            }
            if format_segment && character == '}' && nested_format > 0 {
                nested_format -= 1;
                segment.push(character);
                self.offset += character.len_utf8();
                continue;
            }
            if character == '/' && nested_format == 0 {
                self.offset += 1;
                return Ok(segment);
            }
            segment.push(character);
            self.offset += character.len_utf8();
        }
        Err("The server returned an unclosed snippet transform".into())
    }

    fn append_unknown_variable(&mut self, name: &str, placeholder: &str) {
        let start = self.text.len();
        self.text.push_str(placeholder);
        self.unresolved_variables
            .push((name.into(), start..self.text.len()));
    }

    fn skip_sequence(&mut self) -> Result<(), String> {
        let mut nested = 0usize;
        while let Some(character) = self.peek() {
            match character {
                '\\' => {
                    self.offset += 1;
                    if let Some(escaped) = self.peek() {
                        self.offset += escaped.len_utf8();
                    }
                }
                '$' if self.source[self.offset..].starts_with("${") => {
                    self.offset += 2;
                    nested = nested.saturating_add(1);
                }
                '}' if nested == 0 => {
                    self.offset += 1;
                    return Ok(());
                }
                '}' => {
                    self.offset += 1;
                    nested -= 1;
                }
                character => self.offset += character.len_utf8(),
            }
        }
        Err("The server returned an unclosed snippet variable default".into())
    }

    fn parse_choice(&mut self) -> Result<Vec<String>, String> {
        let mut choices = Vec::new();
        let mut choice = String::new();
        let mut closed_choice = false;

        while let Some(character) = self.peek() {
            match character {
                '\\' => {
                    self.offset += 1;
                    if let Some(escaped) = self.peek() {
                        if matches!(escaped, ',' | '|' | '\\' | '}') {
                            self.offset += escaped.len_utf8();
                            choice.push(escaped);
                        } else {
                            choice.push('\\');
                        }
                    } else {
                        choice.push('\\');
                    }
                }
                ',' => {
                    self.offset += 1;
                    choices.push(std::mem::take(&mut choice));
                }
                '|' if self.source[self.offset..].starts_with("|}") => {
                    self.offset += 2;
                    choices.push(std::mem::take(&mut choice));
                    closed_choice = true;
                    break;
                }
                '|' => {
                    choice.push('|');
                    self.offset += 1;
                }
                character => {
                    choice.push(character);
                    self.offset += character.len_utf8();
                }
            }
        }

        if closed_choice {
            if let Some(first_choice) = choices.first() {
                self.text.push_str(first_choice);
            }
            Ok(choices)
        } else {
            Err("The server returned an unclosed snippet choice".into())
        }
    }

    fn parse_escape(&mut self) {
        self.offset += 1;
        match self.peek() {
            Some(character @ ('$' | '}' | '\\')) => {
                self.text.push(character);
                self.offset += character.len_utf8();
            }
            _ => self.text.push('\\'),
        }
    }

    fn parse_index(&mut self) -> Result<Option<u32>, String> {
        let start = self.offset;
        while self
            .peek()
            .is_some_and(|character| character.is_ascii_digit())
        {
            self.offset += 1;
        }
        if self.offset == start {
            return Ok(None);
        }
        self.source[start..self.offset]
            .parse()
            .map(Some)
            .map_err(|_| {
                "The server returned an invalid snippet tabstop index".into()
            })
    }

    fn add_tabstop(&mut self, index: u32, range: Range<usize>) {
        let choices = self.choices.remove(&index).unwrap_or_default();
        let tabstop = self.tabstops.entry(index).or_insert_with(|| Tabstop {
            index,
            ranges: Vec::new(),
            choices: Vec::new(),
        });
        if tabstop.choices.is_empty() {
            tabstop.choices = choices;
        }
        tabstop.ranges.push(range);
    }

    fn add_tabstop_or_default(&mut self, index: u32) {
        let start = self.text.len();
        if let Some(default) = self.tabstop_defaults.get(&index) {
            self.text.push_str(default);
        }
        self.add_tabstop(index, start..self.text.len());
    }

    fn expand_forward_tabstop_references(&mut self) -> Result<(), String> {
        for (index, default) in &self.tabstop_defaults {
            if *index == 0 || default.is_empty() {
                continue;
            }
            loop {
                let target = self.tabstops.get(index).and_then(|tabstop| {
                    tabstop
                        .ranges
                        .iter()
                        .filter(|range| range.is_empty())
                        .max_by_key(|range| range.start)
                        .cloned()
                });
                let Some(target) = target else {
                    break;
                };
                let end = target
                    .start
                    .checked_add(default.len())
                    .ok_or("The server returned an oversized snippet")?;
                self.text.insert_str(target.start, default);
                for (tabstop_index, tabstop) in &mut self.tabstops {
                    for range in &mut tabstop.ranges {
                        if *tabstop_index == *index && *range == target {
                            *range = target.start..end;
                        } else if range.start >= target.start {
                            range.start = range
                                .start
                                .checked_add(default.len())
                                .ok_or("The server returned an oversized snippet")?;
                            range.end = range
                                .end
                                .checked_add(default.len())
                                .ok_or("The server returned an oversized snippet")?;
                        } else if range.end > target.start {
                            range.end = range
                                .end
                                .checked_add(default.len())
                                .ok_or("The server returned an oversized snippet")?;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn consume(&mut self, character: char) -> bool {
        if self.peek() == Some(character) {
            self.offset += character.len_utf8();
            true
        } else {
            false
        }
    }

    fn peek(&self) -> Option<char> {
        self.source.get(self.offset..)?.chars().next()
    }
}

fn apply_transform(
    value: &str,
    pattern: &str,
    format: &str,
    options: &str,
) -> Result<String, String> {
    let mut global = false;
    let mut builder = fancy_regex::RegexBuilder::new(pattern);
    builder.backtrack_limit(100_000);
    for option in options.chars() {
        match option {
            'g' => global = true,
            'i' => {
                builder.case_insensitive(true);
            }
            'm' => {
                builder.multi_line(true);
            }
            's' => {
                builder.dot_matches_new_line(true);
            }
            'u' => {
                builder.unicode_mode(true);
            }
            _ => {
                return Err(
                    "The server returned an unsupported snippet transform option"
                        .into(),
                );
            }
        }
    }
    let regex = builder.build().map_err(|_| {
        "The server returned an invalid snippet transform regex".to_string()
    })?;
    let mut output = String::with_capacity(value.len());
    let mut copied_through = 0;
    let mut matched = false;
    for captures in regex.captures_iter(value) {
        let captures = captures.map_err(|_| {
            "The server returned a snippet transform that exceeded its execution limit"
                .to_string()
        })?;
        let Some(full_match) = captures.get(0) else {
            continue;
        };
        output.push_str(&value[copied_through..full_match.start()]);
        output.push_str(&render_transform_format(format, &captures)?);
        copied_through = full_match.end();
        matched = true;
        if !global {
            break;
        }
    }
    if !matched {
        return Ok(value.to_owned());
    }
    output.push_str(&value[copied_through..]);
    Ok(output)
}

fn render_transform_format(
    format: &str,
    captures: &fancy_regex::Captures<'_, str>,
) -> Result<String, String> {
    let mut output = String::with_capacity(format.len());
    let mut offset = 0;
    while offset < format.len() {
        let Some(character) = format[offset..].chars().next() else {
            break;
        };
        if character == '\\' {
            offset += 1;
            if let Some(escaped @ ('$' | '}' | '\\')) =
                format[offset..].chars().next()
            {
                output.push(escaped);
                offset += escaped.len_utf8();
            } else {
                output.push('\\');
            }
            continue;
        }
        if character != '$' {
            output.push(character);
            offset += character.len_utf8();
            continue;
        }

        let next_offset = offset + character.len_utf8();
        if format[next_offset..].starts_with('{') {
            let expression_start = next_offset + 1;
            let expression_end =
                find_format_expression_end(format, expression_start).ok_or(
                    "The server returned an invalid snippet transform format",
                )?;
            output.push_str(&render_braced_format(
                &format[expression_start..expression_end],
                captures,
            )?);
            offset = expression_end + 1;
        } else {
            let mut index_end = next_offset;
            while let Some(character) = format[index_end..].chars().next() {
                if !character.is_ascii_digit() {
                    break;
                }
                index_end += character.len_utf8();
            }
            if index_end == next_offset {
                output.push('$');
                offset = next_offset;
                continue;
            }
            let index =
                format[next_offset..index_end]
                    .parse::<usize>()
                    .map_err(|_| {
                        "The server returned an invalid snippet transform capture"
                            .to_string()
                    })?;
            output.push_str(capture_value(captures, index));
            offset = index_end;
        }
    }
    Ok(output)
}

fn find_format_expression_end(format: &str, start: usize) -> Option<usize> {
    let mut offset = start;
    let mut nested = 0usize;
    while offset < format.len() {
        if format[offset..].starts_with("\\}") {
            offset += 2;
        } else if format[offset..].starts_with("${") {
            nested += 1;
            offset += 2;
        } else if format[offset..].starts_with('}') {
            if nested == 0 {
                return Some(offset);
            }
            nested -= 1;
            offset += 1;
        } else if let Some(character) = format[offset..].chars().next() {
            offset += character.len_utf8();
        } else {
            return None;
        }
    }
    None
}

fn render_braced_format(
    expression: &str,
    captures: &fancy_regex::Captures<'_, str>,
) -> Result<String, String> {
    let index_end = expression
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(expression.len());
    if index_end == 0 {
        return Err(
            "The server returned an invalid snippet transform capture".into()
        );
    }
    let index = expression[..index_end].parse::<usize>().map_err(|_| {
        "The server returned an invalid snippet transform capture".to_string()
    })?;
    let value = capture_value(captures, index);
    let modifier = &expression[index_end..];
    if modifier.is_empty() {
        return Ok(value.to_owned());
    }
    if let Some(operation) = modifier.strip_prefix(":/") {
        return match operation {
            "upcase" => Ok(value.to_uppercase()),
            "downcase" => Ok(value.to_lowercase()),
            "capitalize" => {
                let mut characters = value.chars();
                let Some(first) = characters.next() else {
                    return Ok(String::new());
                };
                let mut capitalized = first.to_uppercase().to_string();
                capitalized.push_str(characters.as_str());
                Ok(capitalized)
            }
            _ => Err(
                "The server returned an unsupported snippet transform modifier"
                    .into(),
            ),
        };
    }
    if let Some(alternative) = modifier.strip_prefix(":+") {
        return if value.is_empty() {
            Ok(String::new())
        } else {
            render_transform_format(alternative, captures)
        };
    }
    if let Some(alternative) = modifier.strip_prefix(":-") {
        return if value.is_empty() {
            render_transform_format(alternative, captures)
        } else {
            Ok(String::new())
        };
    }
    if let Some(alternatives) = modifier.strip_prefix(":?") {
        let Some(separator) = find_top_level_colon(alternatives) else {
            return Err(
                "The server returned an invalid conditional snippet transform"
                    .into(),
            );
        };
        let (when_set, when_empty) = alternatives.split_at(separator);
        let when_empty = &when_empty[1..];
        return if value.is_empty() {
            render_transform_format(when_empty, captures)
        } else {
            render_transform_format(when_set, captures)
        };
    }
    if let Some(default) = modifier.strip_prefix(':') {
        return render_transform_format(default, captures);
    }
    Err("The server returned an invalid snippet transform modifier".into())
}

fn find_top_level_colon(value: &str) -> Option<usize> {
    let mut offset = 0;
    let mut nested = 0usize;
    while offset < value.len() {
        if value[offset..].starts_with("\\:") {
            offset += 2;
        } else if value[offset..].starts_with("${") {
            nested += 1;
            offset += 2;
        } else if value[offset..].starts_with('}') && nested > 0 {
            nested -= 1;
            offset += 1;
        } else if value[offset..].starts_with(':') && nested == 0 {
            return Some(offset);
        } else if let Some(character) = value[offset..].chars().next() {
            offset += character.len_utf8();
        } else {
            return None;
        }
    }
    None
}

fn capture_value<'a>(
    captures: &fancy_regex::Captures<'a, str>,
    index: usize,
) -> &'a str {
    captures.get(index).map_or("", |capture| capture.as_str())
}

fn is_variable_start(character: char) -> bool {
    character.is_ascii_alphabetic() || character == '_'
}

fn is_variable_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '_'
}

fn contains_edit(range: &Range<usize>, edit: &Range<usize>) -> bool {
    range.start <= edit.start && edit.end <= range.end
}

fn text_diff(old: &str, new: &str) -> Option<(Range<usize>, Range<usize>)> {
    if old == new {
        return None;
    }
    let prefix_bytes = old
        .chars()
        .zip(new.chars())
        .take_while(|(old, new)| old == new)
        .map(|(character, _)| character.len_utf8())
        .sum::<usize>();
    let old_tail = &old[prefix_bytes..];
    let new_tail = &new[prefix_bytes..];
    let suffix_bytes = old_tail
        .chars()
        .rev()
        .zip(new_tail.chars().rev())
        .take_while(|(old, new)| old == new)
        .map(|(character, _)| character.len_utf8())
        .sum::<usize>();
    Some((
        prefix_bytes..old.len() - suffix_bytes,
        prefix_bytes..new.len() - suffix_bytes,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ordered_nested_and_final_tabstops() {
        let snippet =
            Snippet::parse("call(${2:value}, ${1:item})$0").expect("valid snippet");
        assert_eq!(snippet.text, "call(value, item)");
        assert_eq!(
            snippet.tabstops,
            vec![
                Tabstop {
                    index: 1,
                    ranges: vec![12..16],
                    choices: Vec::new(),
                },
                Tabstop {
                    index: 2,
                    ranges: vec![5..10],
                    choices: Vec::new(),
                },
                Tabstop {
                    index: 0,
                    ranges: vec![17..17],
                    choices: Vec::new(),
                },
            ]
        );
    }

    #[test]
    fn parses_nested_placeholders_and_repeated_ranges() {
        let snippet = Snippet::parse("${1:outer ${2:inner}} $1")
            .expect("valid nested snippet");
        assert_eq!(snippet.text, "outer inner outer inner");
        assert_eq!(snippet.tabstops[0].index, 1);
        assert_eq!(snippet.tabstops[0].ranges, vec![0..11, 12..23]);
        assert_eq!(snippet.tabstops[1].index, 2);
        assert_eq!(snippet.tabstops[1].ranges, vec![6..11]);
    }

    #[test]
    fn repeated_choice_tabstops_expand_to_the_selected_default() {
        let snippet =
            Snippet::parse("${1|left,right|}-$1").expect("valid repeated choice");
        assert_eq!(snippet.text, "left-left");
        assert_eq!(snippet.tabstops[0].ranges, vec![0..4, 5..9]);
        assert_eq!(
            snippet.tabstops[0].choices,
            vec!["left".to_string(), "right".to_string()]
        );
    }

    #[test]
    fn forward_tabstop_references_expand_and_rebase_nested_ranges() {
        let snippet = Snippet::parse("${1} ${1:outer ${2:inner}}")
            .expect("valid forward tabstop reference");
        assert_eq!(snippet.text, "outer inner outer inner");
        assert_eq!(snippet.tabstops[0].ranges, vec![0..11, 12..23]);
        assert_eq!(snippet.tabstops[1].ranges, vec![18..23]);
    }

    #[test]
    fn parses_choices_and_escapes() {
        let snippet = Snippet::parse(r#"${1|one,two\,too|} \$ \} \\ end"#)
            .expect("valid choices and escapes");
        assert_eq!(snippet.text, "one $ } \\ end");
        assert_eq!(snippet.tabstops[0].ranges, vec![0..3]);
        assert_eq!(
            snippet.tabstops[0].choices,
            vec!["one".to_string(), "two,too".to_string()]
        );
    }

    #[test]
    fn snippet_choice_cycles_and_repositions_later_tabstops() {
        let snippet = Snippet::parse("${1|first,🦀,long-value|}-$2")
            .expect("valid choice snippet");
        let mut session =
            Session::new(snippet.text, snippet.tabstops).expect("editable snippet");

        assert_eq!(session.active_choices().len(), 3);
        assert_eq!(session.active_choices()[1], "🦀");
        assert_eq!(session.cycle_choice(true), Some((0..5, 0..4, "🦀".into())));
        assert_eq!(session.active_range(), Some(0..4));
        assert_eq!(session.move_to(1), Some(5..5));

        let snippet =
            Snippet::parse("${1|first,second,last|}").expect("valid choice snippet");
        let mut session =
            Session::new(snippet.text, snippet.tabstops).expect("editable snippet");
        assert_eq!(
            session.cycle_choice(false),
            Some((0..5, 0..4, "last".into()))
        );
    }

    #[test]
    fn undoing_a_choice_change_restores_the_selected_option() {
        let snippet =
            Snippet::parse("${1|first,second|}").expect("valid choice snippet");
        let mut session =
            Session::new(snippet.text, snippet.tabstops).expect("editable snippet");
        session.cycle_choice(true).expect("select second option");

        assert!(session.update_after_edit("first".into(), 0..5));
        assert_eq!(session.active_choice_index(), 0);
        assert_eq!(session.active_choices().len(), 2);
        assert_eq!(session.active_choices()[0], "first");
    }

    #[test]
    fn adds_a_final_caret_for_plain_text_or_trailing_placeholder() {
        assert_eq!(
            Snippet::parse("plain").expect("plain text").tabstops,
            vec![Tabstop {
                index: 0,
                ranges: vec![5..5],
                choices: Vec::new(),
            }]
        );
        assert_eq!(
            Snippet::parse("plain$1").expect("trailing stop").tabstops,
            vec![Tabstop {
                index: 1,
                ranges: vec![5..5],
                choices: Vec::new(),
            }]
        );
    }

    #[test]
    fn resolves_variables_and_uses_defaults_when_values_are_unavailable() {
        let variables = Variables::default();
        let snippet = Snippet::parse_with_variables(
            "${TM_FILENAME}-${UNKNOWN:default}-${UNKNOWN:${1:nested}}-$TM_FILENAME",
            &variables,
        )
        .expect("valid snippet variables");
        assert_eq!(snippet.text, "TM_FILENAME-default-nested-TM_FILENAME");
        assert_eq!(snippet.tabstops[0].index, 1);
        assert_eq!(snippet.tabstops[1].index, 2);
        assert_eq!(snippet.tabstops[1].ranges, vec![0..11, 27..38]);

        let variables =
            Variables(BTreeMap::from([("TM_FILENAME".into(), "main.rs".into())]));
        let snippet = Snippet::parse_with_variables(
            "${TM_FILENAME:ignored}-${UNKNOWN}",
            &variables,
        )
        .expect("known and unknown variables");
        assert_eq!(snippet.text, "main.rs-UNKNOWN");
        assert_eq!(snippet.tabstops[0].ranges, vec![8..15]);
        assert_eq!(
            Snippet::parse_with_variables("${TM_FILENAME/.*//}", &variables)
                .expect("valid empty replacement transform")
                .text,
            ""
        );
    }

    #[test]
    fn completion_variables_reflect_unicode_buffer_file_and_selection() {
        let text = "let naïve = \"x\";\nfn main() {}\n";
        let cursor = text.find("naïve").expect("word") + "naïve".len();
        let variables = Variables::for_completion(
            text,
            cursor,
            0,
            Path::new("/workspace/src/main.rs"),
            Path::new("/workspace"),
            "chosen",
            "copied",
            None,
        );
        let snippet = Snippet::parse_with_variables(
            "${TM_FILENAME_BASE}|${RELATIVE_FILEPATH}|${WORKSPACE_NAME}|${WORKSPACE_FOLDER}|${CLIPBOARD}|${TM_LINE_INDEX}|${TM_LINE_NUMBER}|${TM_CURRENT_LINE}|${TM_CURRENT_WORD}|${TM_SELECTED_TEXT}",
            &variables,
        )
        .expect("completion variables");
        assert_eq!(
            snippet.text,
            "main|src/main.rs|workspace|/workspace|copied|0|1|let naïve = \"x\";|naïve|chosen"
        );
    }

    #[test]
    fn completion_variables_include_builtin_language_comments() {
        let variables = Variables::for_completion(
            "",
            0,
            0,
            Path::new("/workspace/src/main.rs"),
            Path::new("/workspace"),
            "",
            "",
            None,
        );
        let rust = Snippet::parse_with_variables(
            "$LINE_COMMENT|$BLOCK_COMMENT_START|$BLOCK_COMMENT_END",
            &variables,
        )
        .expect("Rust comment variables");
        assert_eq!(rust.text, "//|/*|*/");

        let variables = Variables::for_completion(
            "",
            0,
            0,
            Path::new("/workspace/src/main.ts"),
            Path::new("/workspace"),
            "",
            "",
            None,
        );
        let typescript = Snippet::parse_with_variables(
            "$LINE_COMMENT|$BLOCK_COMMENT_START|$BLOCK_COMMENT_END",
            &variables,
        )
        .expect("TypeScript comment variables");
        assert_eq!(typescript.text, "//|/*|*/");

        let variables = Variables::for_completion(
            "",
            0,
            0,
            Path::new("/workspace/main.py"),
            Path::new("/workspace"),
            "",
            "",
            None,
        );
        let python = Snippet::parse_with_variables("$LINE_COMMENT", &variables)
            .expect("Python line comment variable");
        assert_eq!(python.text, "#");

        let variables = Variables::for_completion(
            "",
            0,
            0,
            Path::new("/workspace/index.html"),
            Path::new("/workspace"),
            "",
            "",
            None,
        );
        let html = Snippet::parse_with_variables(
            "$BLOCK_COMMENT_START Hello $BLOCK_COMMENT_END",
            &variables,
        )
        .expect("HTML block comment variables");
        assert_eq!(html.text, "<!-- Hello -->");
    }

    #[test]
    fn extension_comment_metadata_overrides_the_builtin_fallback() {
        let extension_comments = CommentVariables {
            line_comment: Some("## ".into()),
            block_comment: Some(("{-".into(), "-}".into())),
        };
        let variables = Variables::for_completion(
            "",
            0,
            0,
            Path::new("/workspace/main.ts"),
            Path::new("/workspace"),
            "",
            "",
            Some(&extension_comments),
        );
        let snippet = Snippet::parse_with_variables(
            "$LINE_COMMENT|$BLOCK_COMMENT_START|$BLOCK_COMMENT_END",
            &variables,
        )
        .expect("extension comment variables");
        assert_eq!(snippet.text, "## |{-|-}");
    }

    #[test]
    fn partial_extension_comment_metadata_keeps_builtin_delimiters() {
        let extension_comments = CommentVariables {
            line_comment: Some("# ".into()),
            block_comment: None,
        };
        let variables = Variables::for_completion(
            "",
            0,
            0,
            Path::new("/workspace/main.ts"),
            Path::new("/workspace"),
            "",
            "",
            Some(&extension_comments),
        );
        let snippet = Snippet::parse_with_variables(
            "$LINE_COMMENT|$BLOCK_COMMENT_START|$BLOCK_COMMENT_END",
            &variables,
        )
        .expect("partial extension comment variables");
        assert_eq!(snippet.text, "# |/*|*/");
    }

    #[test]
    fn completion_variables_include_date_random_and_uuid_values() {
        let variables = Variables::for_completion(
            "",
            0,
            0,
            Path::new("/workspace/main.rs"),
            Path::new("/workspace"),
            "",
            "",
            None,
        );
        let snippet = Snippet::parse_with_variables(
            "${CURSOR_INDEX}|${CURSOR_NUMBER}|${CURRENT_YEAR}|${CURRENT_YEAR_SHORT}|${CURRENT_MONTH}|${CURRENT_DATE}|${CURRENT_HOUR}|${CURRENT_MINUTE}|${CURRENT_SECOND}|${CURRENT_MILLISECOND}|${CURRENT_SECONDS_UNIX}|${CURRENT_MILLISECONDS_UNIX}|${CURRENT_TIMEZONE_OFFSET}|${RANDOM}|${RANDOM_HEX}|${UUID}",
            &variables,
        )
        .expect("dynamic completion variables");
        let fields = snippet.text.split('|').collect::<Vec<_>>();
        assert_eq!(fields.first(), Some(&"0"));
        assert_eq!(fields.get(1), Some(&"1"));
        assert_eq!(fields.get(2).map(|field| field.len()), Some(4));
        assert_eq!(fields.get(3).map(|field| field.len()), Some(2));
        assert!(fields.get(4).is_some_and(|field| field.len() == 2));
        assert!(fields.get(5).is_some_and(|field| field.len() == 2));
        assert!(fields.get(6).is_some_and(|field| field.len() == 2));
        assert!(fields.get(7).is_some_and(|field| field.len() == 2));
        assert!(fields.get(8).is_some_and(|field| field.len() == 2));
        assert!(fields.get(9).is_some_and(|field| field.len() == 3));
        assert!(
            fields
                .get(10)
                .is_some_and(|field| field.parse::<i64>().is_ok())
        );
        assert!(
            fields
                .get(11)
                .is_some_and(|field| field.parse::<i64>().is_ok())
        );
        assert!(fields.get(12).is_some_and(|field| {
            (field.starts_with('+') || field.starts_with('-')) && field.contains(':')
        }));
        assert!(fields.get(13).is_some_and(|field| field.len() == 6));
        assert!(fields.get(14).is_some_and(|field| field.len() == 6
            && field.chars().all(|character| character.is_ascii_hexdigit())));
        assert!(
            fields
                .get(15)
                .is_some_and(|field| uuid::Uuid::parse_str(field).is_ok())
        );
    }

    #[test]
    fn variable_transforms_support_captures_case_flags_and_conditionals() {
        let variables = Variables(BTreeMap::from([
            ("TM_FILENAME".into(), "main.rs".into()),
            ("TM_CURRENT_WORD".into(), "main".into()),
        ]));
        let snippet = Snippet::parse_with_variables(
            r"${TM_FILENAME/([a-z]+)\.([a-z]+)/${1:/capitalize}.${2:/upcase}/}-${TM_FILENAME/[a-z]/x/gi}-${TM_CURRENT_WORD/([a-z]+)/${1:+has}-${1:-empty}-${1:?yes:no}/}-${TM_FILENAME/(?<=main)\.rs$/.txt/}",
            &variables,
        )
        .expect("valid variable transforms");
        assert_eq!(snippet.text, "Main.RS-xxxx.xx-has--yes-main.txt");
    }

    #[test]
    fn unknown_transformed_variables_remain_editable_and_no_match_is_unchanged() {
        let snippet = Snippet::parse_with_variables(
            r"${UNKNOWN/([A-Z]+)/$1/}-${TM_FILENAME/never/match/}",
            &Variables(BTreeMap::from([("TM_FILENAME".into(), "main.rs".into())])),
        )
        .expect("valid variable transforms");
        assert_eq!(snippet.text, "UNKNOWN-main.rs");
        assert_eq!(snippet.tabstops[0].ranges, vec![0..7]);
    }

    #[test]
    fn rejects_unclosed_placeholders() {
        assert!(Snippet::parse("${1:unfinished").is_err());
    }

    #[test]
    fn session_tracks_edits_and_moves_between_placeholders() {
        let snippet = Snippet::parse("${1:name}.${2:member}$0").expect("snippet");
        let mut session = Session::new(snippet.text.clone(), snippet.tabstops)
            .expect("editable snippet");
        assert_eq!(session.active_range(), Some(0..4));
        assert!(session.update_after_edit("long_name.member".into(), 9..9));
        assert_eq!(session.active_range(), Some(0..9));
        assert_eq!(session.move_to(1), Some(10..16));
        assert_eq!(session.move_to(2), Some(16..16));
    }

    #[test]
    fn session_ends_when_edit_leaves_the_active_placeholder() {
        let snippet = Snippet::parse("${1:name}.${2:member}$0").expect("snippet");
        let mut session = Session::new(snippet.text.clone(), snippet.tabstops)
            .expect("editable snippet");
        assert!(!session.update_after_edit("x.member".into(), 8..8));
    }
}
