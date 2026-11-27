use std::{
    fs::{self, File, Metadata},
    io::{self, BufRead, BufReader, Seek, SeekFrom},
    ops::Range,
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};

use anyhow::{Context, Result, bail};
use unicode_segmentation::GraphemeCursor;
use unicode_width::UnicodeWidthStr;

use super::worker::Progress;

const BLOCK: usize = 64 * 1024;
const CHECKPOINT_INTERVAL: u64 = 4 * 1024 * 1024;
// The old screen and its replacement together stay below the 8 MiB text budget.
const PAGE_BUDGET: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Hit {
    pub query: String,
    pub line: u64,
    pub bytes: Range<u64>,
    pub column: u64,
    pub end_column: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct View {
    pub selected: u64,
    pub top: u64,
    pub left: u64,
    pub height: usize,
    pub width: usize,
    pub hit: Option<Hit>,
}

#[derive(Debug)]
pub(crate) struct Row {
    pub number: u64,
    pub text: String,
    pub highlight: Option<Range<usize>>,
    pub mapping: Vec<DisplayRange>,
}

#[derive(Debug)]
pub(crate) struct DisplayRange {
    pub source: Range<u64>,
    pub display: Range<usize>,
}

#[derive(Debug, Clone)]
pub(crate) struct Page {
    pub view: View,
    pub rows: Arc<[Row]>,
    pub total: Option<u64>,
    pub notice: Option<String>,
}

#[derive(Clone, Copy)]
struct Checkpoint {
    offset: u64,
    line: u64,
    line_start: u64,
    column: u64,
}

struct Token {
    ch: char,
    offset: u64,
    end: u64,
    line: u64,
    column: u64,
    end_column: u64,
    grapheme: Range<u64>,
    first: bool,
}

pub(super) struct Source {
    path: PathBuf,
    reader: BufReader<File>,
    length: u64,
    modified: Option<SystemTime>,
    index: Vec<Checkpoint>,
    total: Option<u64>,
}

impl Source {
    pub fn open(path: &Path) -> Result<Self> {
        let metadata = fs::symlink_metadata(path)?;
        if !metadata.file_type().is_file() {
            bail!("Only regular files can be viewed");
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                bail!("Links and reparse points cannot be viewed");
            }
        }
        let file =
            File::open(path).with_context(|| format!("Failed to open '{}'", path.display()))?;
        let opened = file.metadata()?;
        if !opened.is_file() || !same_metadata(&metadata, &opened) {
            bail!("File changed while opening; close and reopen it");
        }
        Ok(Self {
            path: path.to_owned(),
            reader: BufReader::with_capacity(BLOCK, file),
            length: opened.len(),
            modified: opened.modified().ok(),
            index: vec![Checkpoint {
                offset: 0,
                line: 1,
                line_start: 0,
                column: 0,
            }],
            total: None,
        })
    }

    pub(super) fn unchanged(&self) -> Result<()> {
        for metadata in [
            self.reader.get_ref().metadata()?,
            fs::symlink_metadata(&self.path)?,
        ] {
            if !metadata.is_file()
                || metadata.len() != self.length
                || metadata.modified().ok() != self.modified
            {
                bail!("File changed; close and reopen it");
            }
        }
        Ok(())
    }

    fn checkpoint(&self, line: u64, column: u64) -> Checkpoint {
        self.index
            .iter()
            .rev()
            .find(|entry| entry.line < line || entry.line == line && entry.column <= column)
            .copied()
            .unwrap_or(self.index[0])
    }

    fn scan(
        &mut self,
        start: Checkpoint,
        progress: &Progress,
        id: u64,
        mut visit: impl FnMut(Token) -> Result<bool>,
    ) -> Result<()> {
        self.unchanged()?;
        self.reader.seek(SeekFrom::Start(start.offset))?;
        let mut position = start.offset;
        let mut line = start.line;
        let mut line_start = start.line_start;
        let mut column = start.column;
        let mut next_check = position;
        let mut lookahead = None;
        let mut grapheme = String::new();
        while position < self.length || lookahead.is_some() {
            if position >= next_check {
                progress.check(id, position, self.length)?;
                self.unchanged()?;
                next_check = position.saturating_add(BLOCK as u64);
            }
            let bytes = read_grapheme(
                &mut self.reader,
                &mut position,
                self.length,
                &mut lookahead,
                &mut grapheme,
            )?;
            let crlf = grapheme == "\r\n";
            let text = if crlf { "\n" } else { grapheme.as_str() };
            let offset = bytes.start + u64::from(crlf);
            let end_column = column.saturating_add(cell_width(text, column));
            for (index, ch) in text.char_indices() {
                let offset = offset + index as u64;
                if !visit(Token {
                    ch,
                    offset,
                    end: offset + ch.len_utf8() as u64,
                    line,
                    column,
                    end_column,
                    grapheme: bytes.clone(),
                    first: index == 0,
                })? {
                    return Ok(());
                }
            }
            if text == "\n" {
                line = line.saturating_add(1);
                line_start = bytes.end;
                column = 0;
            } else {
                column = end_column;
            }
            if bytes
                .end
                .saturating_sub(self.index.last().map_or(0, |entry| entry.offset))
                >= CHECKPOINT_INTERVAL
            {
                self.index.push(Checkpoint {
                    offset: bytes.end,
                    line,
                    line_start,
                    column,
                });
            }
        }
        self.total = Some(if self.length > 0 && line_start == self.length {
            line.saturating_sub(1).max(1)
        } else {
            line
        });
        self.unchanged()?;
        progress.check(id, position, self.length)?;
        Ok(())
    }

    pub fn last_line(&mut self, progress: &Progress, id: u64) -> Result<u64> {
        if self.total.is_none() {
            let start = *self.index.last().unwrap_or(&self.index[0]);
            self.scan(start, progress, id, |_| Ok(true))?;
        }
        self.total.context("Could not determine the last line")
    }

    pub fn page(&mut self, mut view: View, progress: &Progress, id: u64) -> Result<Page> {
        self.unchanged()?;
        let requested = view.selected.max(1);
        view.selected = requested;
        view.height = view.height.max(1);
        view.width = view.width.max(1);
        if let Some(total) = self.total {
            view.selected = view.selected.min(total);
        }
        view.top = view.top.max(1).min(view.selected);
        if view.selected.saturating_sub(view.top) >= view.height as u64 {
            view.top = view.selected.saturating_sub(view.height as u64 - 1);
        }
        if let Some(total) = self.total {
            view.top = view
                .top
                .min(total.saturating_sub(view.height as u64 - 1).max(1));
        }
        let last = view.top.saturating_add(view.height as u64);
        let mut rows: Vec<Row> = Vec::new();
        let mut stored = 0;
        self.scan(
            self.checkpoint(view.top, view.left),
            progress,
            id,
            |token| {
                if token.line >= last {
                    return Ok(false);
                }
                if token.line < view.top {
                    return Ok(true);
                }
                if rows.last().is_none_or(|row| row.number != token.line) {
                    rows.push(Row {
                        number: token.line,
                        text: String::new(),
                        highlight: None,
                        mapping: Vec::new(),
                    });
                }
                if token.ch == '\n' {
                    return Ok(true);
                }
                let cells = token.end_column.saturating_sub(token.column);
                let right = view.left.saturating_add(view.width as u64);
                let end_column = token.end_column;
                if end_column <= view.left
                    || token.column > right
                    || token.column == right && cells > 0
                {
                    return Ok(true);
                }
                let clipped = token.column < view.left || end_column > right;
                let text = if token.ch == '\t' || clipped {
                    if !token.first {
                        return Ok(true);
                    }
                    " ".repeat((end_column.min(right) - token.column.max(view.left)) as usize)
                } else if token.ch.is_control() {
                    "�".into()
                } else {
                    token.ch.to_string()
                };
                stored += text.len();
                if stored > PAGE_BUDGET {
                    bail!(
                        "Viewport text exceeds the display cache budget; reduce the terminal size"
                    );
                }
                if let Some(row) = rows.last_mut() {
                    let begin = row.text.len();
                    row.text.push_str(&text);
                    let display = begin..row.text.len();
                    if let Some(previous) = row.mapping.last_mut()
                        && previous.source.end == token.offset
                        && previous.display.end == begin
                        && previous.source.end - previous.source.start
                            == (previous.display.end - previous.display.start) as u64
                        && token.end - token.offset == text.len() as u64
                    {
                        previous.source.end = token.end;
                        previous.display.end = row.text.len();
                    } else {
                        row.mapping.push(DisplayRange {
                            source: token.offset..token.end,
                            display,
                        });
                    }
                    if view.hit.as_ref().is_some_and(|hit| {
                        token.grapheme.start < hit.bytes.end && token.grapheme.end > hit.bytes.start
                    }) {
                        let range = row.highlight.get_or_insert(begin..begin);
                        range.end = row.text.len();
                    }
                }
                Ok(true)
            },
        )?;
        if let Some(total) = self.total {
            let top = view
                .top
                .min(total.saturating_sub(view.height as u64 - 1).max(1));
            if view.selected > total || view.top != top {
                view.selected = view.selected.min(total);
                view.top = top;
                drop(rows);
                let mut page = self.page(view, progress, id)?;
                if requested > total {
                    page.notice = Some(format!("Requested line {requested}; last line is {total}"));
                }
                return Ok(page);
            }
            if rows.is_empty() && view.top <= total {
                rows.push(Row {
                    number: view.top,
                    text: String::new(),
                    highlight: None,
                    mapping: Vec::new(),
                });
            }
        }
        Ok(Page {
            view,
            rows: rows.into(),
            total: self.total,
            notice: self
                .total
                .filter(|&total| requested > total)
                .map(|total| format!("Requested line {requested}; last line is {total}")),
        })
    }

    pub fn find(
        &mut self,
        query: &str,
        line: u64,
        byte: Option<u64>,
        backwards: bool,
        progress: &Progress,
        id: u64,
    ) -> Result<Option<Hit>> {
        let pattern: Vec<char> = query.chars().collect();
        if pattern.is_empty() || pattern.iter().any(|ch| matches!(ch, '\r' | '\n')) {
            bail!("Enter a nonempty single-line search query");
        }
        let mut prefix = vec![0; pattern.len()];
        let mut matched = 0;
        for i in 1..pattern.len() {
            while matched > 0 && pattern[i] != pattern[matched] {
                matched = prefix[matched - 1];
            }
            if pattern[i] == pattern[matched] {
                matched += 1;
            }
            prefix[i] = matched;
        }
        let mut starts = std::collections::VecDeque::new();
        let mut found = None;
        matched = 0;
        let start = if backwards {
            self.index[0]
        } else {
            let checkpoint = self.checkpoint(line, 0);
            if checkpoint.line == line {
                // Column zero may follow leading zero-width graphemes that still need searching.
                Checkpoint {
                    offset: checkpoint.line_start,
                    column: 0,
                    ..checkpoint
                }
            } else {
                checkpoint
            }
        };
        self.scan(start, progress, id, |token| {
            if backwards
                && (token.line > line
                    || token.line == line
                        && byte
                            .is_none_or(|at| token.offset >= at.saturating_add(query.len() as u64)))
            {
                return Ok(false);
            }
            if !backwards && (token.line < line || byte.is_some_and(|at| token.offset < at)) {
                return Ok(true);
            }
            if token.ch == '\n' {
                matched = 0;
                starts.clear();
                return Ok(true);
            }
            starts.push_back((token.offset, token.line, token.column));
            if starts.len() > pattern.len() {
                starts.pop_front();
            }
            while matched > 0 && token.ch != pattern[matched] {
                matched = prefix[matched - 1];
            }
            if token.ch == pattern[matched] {
                matched += 1;
            }
            if matched == pattern.len() {
                if let Some(&(offset, line, column)) = starts.front()
                    && (!backwards || byte.is_none_or(|at| offset < at))
                {
                    found = Some(Hit {
                        query: query.to_owned(),
                        line,
                        bytes: offset..token.end,
                        column,
                        end_column: token.end_column,
                    });
                }
                matched = prefix[matched - 1];
                if !backwards {
                    return Ok(false);
                }
            }
            Ok(true)
        })?;
        Ok(found)
    }
}

fn same_metadata(a: &Metadata, b: &Metadata) -> bool {
    a.len() == b.len() && a.modified().ok() == b.modified().ok()
}

fn cell_width(text: &str, column: u64) -> u64 {
    if text == "\t" {
        4 - column % 4
    } else if text == "\n" {
        0
    } else if text.chars().any(char::is_control) {
        1
    } else {
        text.width() as u64
    }
}

fn read_grapheme(
    reader: &mut BufReader<File>,
    position: &mut u64,
    length: u64,
    lookahead: &mut Option<(u64, char)>,
    text: &mut String,
) -> Result<Range<u64>> {
    text.clear();
    let (offset, first) = if let Some(next) = lookahead.take() {
        next
    } else {
        let offset = *position;
        (offset, read_char(reader, position)?)
    };
    text.push(first);
    if first.is_control() && first != '\r' {
        return Ok(offset..*position);
    }
    while *position < length {
        let next_offset = *position;
        let next = read_char(reader, position)?;
        let boundary = text.len();
        text.push(next);
        let separate = GraphemeCursor::new(boundary, text.len(), true)
            .is_boundary(text, 0)
            .map_err(|error| anyhow::anyhow!("Failed to determine grapheme boundary: {error:?}"))?;
        // shortcut: split graphemes beyond 64 KiB to keep buffering and cancellation bounded.
        if separate || text.len() > BLOCK {
            text.truncate(boundary);
            *lookahead = Some((next_offset, next));
            return Ok(offset..next_offset);
        }
        if text == "\r\n" {
            break;
        }
    }
    Ok(offset..*position)
}

fn read_char(reader: &mut BufReader<File>, position: &mut u64) -> Result<char> {
    let mut bytes = [0; 4];
    bytes[0] = read_byte(reader)?;
    let count = match bytes[0] {
        0..=0x7f => 1,
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => bail!("File is not valid UTF-8"),
    };
    for byte in &mut bytes[1..count] {
        *byte = read_byte(reader).context("Incomplete UTF-8 character")?;
    }
    let text = std::str::from_utf8(&bytes[..count]).context("File is not valid UTF-8")?;
    *position += count as u64;
    text.chars().next().context("Missing UTF-8 character")
}

fn read_byte(reader: &mut BufReader<File>) -> io::Result<u8> {
    let byte = reader
        .fill_buf()?
        .first()
        .copied()
        .ok_or_else(|| io::Error::from(io::ErrorKind::UnexpectedEof))?;
    reader.consume(1);
    Ok(byte)
}
