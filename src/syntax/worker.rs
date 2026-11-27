use super::{HighlightedPage, StyledRange};
use crate::viewer::Page;
use anyhow::{Context, Result, ensure};
use std::{
    collections::VecDeque,
    fs::{self, File},
    io::{BufRead, BufReader, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    time::{Duration, Instant, SystemTime},
};
use syntect::{
    easy::HighlightLines,
    highlighting::{HighlightState, Theme},
    parsing::{ParseState, SyntaxSet},
};

const LINE_LIMIT: usize = 64 * 1024;
const CONTEXT_LIMIT: u64 = 8 * 1024 * 1024;
const RANGE_LIMIT: usize = 8 * 1024 * 1024 / std::mem::size_of::<StyledRange>();

#[derive(Debug)]
struct Request {
    id: u64,
    page: Arc<Page>,
    theme: Arc<Theme>,
}
#[derive(Debug)]
pub(crate) struct Client {
    requests: SyncSender<Request>,
    results: Receiver<(u64, Result<HighlightedPage, String>)>,
    generation: Arc<AtomicU64>,
    closed: Arc<AtomicBool>,
    pending: Option<Request>,
}

impl Client {
    pub fn start(path: PathBuf) -> std::io::Result<Self> {
        let (requests, incoming) = mpsc::sync_channel::<Request>(1);
        let (outgoing, results) = mpsc::sync_channel(1);
        let generation = Arc::new(AtomicU64::new(0));
        let state = generation.clone();
        let closed = Arc::new(AtomicBool::new(false));
        let done = closed.clone();
        std::thread::Builder::new()
            .name("loom-syntax".into())
            .spawn(move || {
                let mut engine = Engine::open(&path);
                while let Ok(request) = incoming.recv() {
                    let cancelled = || {
                        done.load(Ordering::Relaxed) || state.load(Ordering::Relaxed) != request.id
                    };
                    if cancelled() {
                        continue;
                    }
                    let result = match &mut engine {
                        Ok(engine) => engine.highlight(request.page, &request.theme, &cancelled),
                        Err(error) => Err(anyhow::anyhow!("{error:#}")),
                    }
                    .map_err(|error| format!("{error:#}"));
                    if !cancelled() && outgoing.send((request.id, result)).is_err() {
                        break;
                    }
                }
            })?;
        Ok(Self {
            requests,
            results,
            generation,
            closed,
            pending: None,
        })
    }
    pub fn request(&mut self, page: Arc<Page>, theme: Arc<Theme>) {
        let id = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        self.pending = Some(Request { id, page, theme });
        self.flush();
    }
    fn flush(&mut self) {
        if let Some(request) = self.pending.take()
            && let Err(mpsc::TrySendError::Full(request)) = self.requests.try_send(request)
        {
            self.pending = Some(request);
        }
    }
    pub fn poll(&mut self) -> Option<Result<HighlightedPage, String>> {
        self.flush();
        let mut latest = None;
        while let Ok((id, result)) = self.results.try_recv() {
            if id == self.generation.load(Ordering::Relaxed) {
                latest = Some(result);
            }
        }
        latest
    }
    pub fn cancel(&mut self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
        self.pending = None;
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::Relaxed);
        self.cancel();
    }
}

struct Checkpoint {
    line: u64,
    offset: u64,
    highlight: HighlightState,
    parse: ParseState,
}
struct Engine {
    path: PathBuf,
    reader: BufReader<File>,
    length: u64,
    modified: Option<SystemTime>,
    syntax: Option<usize>,
    checkpoints: VecDeque<Checkpoint>,
    theme: Option<Arc<Theme>>,
}

fn syntaxes() -> &'static SyntaxSet {
    static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
    SYNTAXES.get_or_init(two_face::syntax::extra_newlines)
}

impl Engine {
    fn open(path: &Path) -> Result<Self> {
        let metadata = fs::symlink_metadata(path)?;
        ensure!(
            metadata.file_type().is_file(),
            "Only regular files can be highlighted"
        );
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            ensure!(
                metadata.file_attributes() & 0x400 == 0,
                "Cannot highlight reparse points"
            );
        }
        let file = File::open(path)?;
        let opened = file.metadata()?;
        ensure!(
            opened.len() == metadata.len() && opened.modified().ok() == metadata.modified().ok(),
            "File changed while opening"
        );
        let mut reader = BufReader::with_capacity(64 * 1024, file);
        let mut first = Vec::new();
        read_line(&mut reader, &mut first)?;
        let first = std::str::from_utf8(&first).unwrap_or("");
        let set = syntaxes();
        let syntax = set
            .find_syntax_by_extension(
                path.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or(""),
            )
            .or_else(|| {
                set.find_syntax_by_extension(
                    path.extension()
                        .and_then(|extension| extension.to_str())
                        .unwrap_or(""),
                )
            })
            .or_else(|| set.find_syntax_by_first_line(first))
            .and_then(|syntax| {
                set.syntaxes()
                    .iter()
                    .position(|entry| std::ptr::eq(entry, syntax))
            });
        Ok(Self {
            path: path.into(),
            reader,
            length: opened.len(),
            modified: opened.modified().ok(),
            syntax,
            checkpoints: VecDeque::new(),
            theme: None,
        })
    }
    fn unchanged(&self) -> Result<()> {
        for metadata in [
            self.reader.get_ref().metadata()?,
            fs::symlink_metadata(&self.path)?,
        ] {
            ensure!(
                metadata.is_file()
                    && metadata.len() == self.length
                    && metadata.modified().ok() == self.modified,
                "File changed; close and reopen it"
            );
        }
        Ok(())
    }
    fn highlight(
        &mut self,
        page: Arc<Page>,
        theme: &Arc<Theme>,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<HighlightedPage> {
        self.unchanged()?;
        let Some(syntax) = self.syntax else {
            return Ok(HighlightedPage {
                rows: Vec::new(),
                page,
                notice: None,
            });
        };
        if self
            .theme
            .as_ref()
            .is_none_or(|previous| !Arc::ptr_eq(previous, theme))
        {
            self.checkpoints.clear();
            self.theme = Some(theme.clone());
        }
        let set = syntaxes();
        let checkpoint = self
            .checkpoints
            .iter()
            .filter(|entry| entry.line <= page.view.top)
            .max_by_key(|entry| entry.line);
        let (mut line, mut offset, mut highlighter) = if let Some(checkpoint) = checkpoint {
            (
                checkpoint.line,
                checkpoint.offset,
                HighlightLines::from_state(
                    theme,
                    checkpoint.highlight.clone(),
                    checkpoint.parse.clone(),
                ),
            )
        } else {
            (1, 0, HighlightLines::new(&set.syntaxes()[syntax], theme))
        };
        self.reader.seek(SeekFrom::Start(offset))?;
        let started = Instant::now();
        let begin = offset;
        let mut rows = Vec::with_capacity(page.rows.len());
        let mut bytes = Vec::new();
        let mut count = 0;
        let last = page.rows.last().map_or(page.view.top, |row| row.number);
        while line <= last {
            ensure!(!cancelled(), "Cancelled");
            // A soft limit between complete lines; an in-flight regex cannot
            // be interrupted safely. The text reader is a separate worker.
            ensure!(
                started.elapsed() < Duration::from_secs(2),
                "Syntax time budget exceeded; showing plain text"
            );
            ensure!(
                offset - begin <= CONTEXT_LIMIT,
                "Syntax context exceeds 8 MiB; showing plain text"
            );
            let length = read_line(&mut self.reader, &mut bytes)?;
            if length == 0 {
                break;
            }
            let text = std::str::from_utf8(&bytes).context("Syntax input is not UTF-8")?;
            let tokens = highlighter.highlight_line(text, set)?;
            if let Some(row) = page.rows.iter().find(|row| row.number == line) {
                let mut styled = Vec::new();
                let mut token_offset = offset;
                for (style, text) in tokens {
                    let end = token_offset + text.len() as u64;
                    for mapping in &row.mapping {
                        let start = mapping.source.start.max(token_offset);
                        let stop = mapping.source.end.min(end);
                        if start >= stop {
                            continue;
                        }
                        let same = mapping.source.end - mapping.source.start
                            == (mapping.display.end - mapping.display.start) as u64;
                        let range = if same {
                            mapping.display.start + (start - mapping.source.start) as usize
                                ..mapping.display.start + (stop - mapping.source.start) as usize
                        } else {
                            mapping.display.clone()
                        };
                        if range.is_empty() {
                            continue;
                        }
                        let style = super::style(style, theme);
                        if let Some(previous) =
                            styled.last_mut().filter(|previous: &&mut StyledRange| {
                                previous.style == style && previous.bytes.end == range.start
                            })
                        {
                            previous.bytes.end = range.end;
                        } else {
                            styled.push(StyledRange {
                                bytes: range,
                                style,
                            });
                            count += 1;
                        }
                        ensure!(
                            count <= RANGE_LIMIT,
                            "Syntax ranges exceed 8 MiB; showing plain text"
                        );
                    }
                    token_offset = end;
                }
                rows.push(styled);
            }
            offset += length as u64;
            line += 1;
            if (line - 1) % 256 == 0 {
                let (highlight, parse) = highlighter.state();
                if !self.checkpoints.iter().any(|entry| entry.line == line) {
                    self.checkpoints.push_back(Checkpoint {
                        line,
                        offset,
                        highlight: highlight.clone(),
                        parse: parse.clone(),
                    });
                    while self.checkpoints.len() > 127 {
                        self.checkpoints.pop_front();
                    }
                }
                highlighter = HighlightLines::from_state(theme, highlight, parse);
            }
        }
        self.unchanged()?;
        Ok(HighlightedPage {
            page,
            rows,
            notice: None,
        })
    }
}

// Never allocate an unbounded line just to pass it to the regex engine.
fn read_line(reader: &mut impl BufRead, output: &mut Vec<u8>) -> Result<usize> {
    output.clear();
    loop {
        let buffer = reader.fill_buf()?;
        if buffer.is_empty() {
            break;
        }
        let count = buffer
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(buffer.len(), |at| at + 1);
        ensure!(
            output.len() + count <= LINE_LIMIT,
            "Syntax line exceeds 64 KiB; showing plain text"
        );
        let complete = buffer[count - 1] == b'\n';
        output.extend_from_slice(&buffer[..count]);
        reader.consume(count);
        if complete {
            break;
        }
    }
    Ok(output.len())
}
