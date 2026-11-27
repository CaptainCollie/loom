use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread,
};

use anyhow::{Result, bail};

use super::source::{Page, Source, View};

#[derive(Debug)]
pub(super) struct Progress {
    pub generation: AtomicU64,
    pub offset: AtomicU64,
    pub length: AtomicU64,
    closed: AtomicBool,
}

impl Progress {
    pub fn check(&self, id: u64, offset: u64, length: u64) -> Result<()> {
        if self.closed.load(Ordering::Relaxed) || self.generation.load(Ordering::Relaxed) != id {
            bail!("Cancelled");
        }
        self.offset.store(offset, Ordering::Relaxed);
        self.length.store(length, Ordering::Relaxed);
        Ok(())
    }
}

#[derive(Debug)]
pub(super) enum Command {
    Page(View),
    Last(View),
    Search {
        query: String,
        backwards: bool,
        initial: bool,
        view: View,
    },
}

#[derive(Debug)]
struct Request {
    id: u64,
    command: Command,
}

#[derive(Debug)]
pub(super) struct Response {
    pub id: u64,
    pub result: Result<Option<Arc<Page>>, String>,
}

#[derive(Debug)]
pub(super) struct Client {
    requests: SyncSender<Request>,
    results: Receiver<Response>,
    pub progress: Arc<Progress>,
    pending: Option<Request>,
}

impl Client {
    pub fn start(path: PathBuf) -> std::io::Result<Self> {
        let (requests, incoming) = mpsc::sync_channel::<Request>(1);
        let (outgoing, results) = mpsc::sync_channel(1);
        let progress = Arc::new(Progress {
            generation: AtomicU64::new(0),
            offset: AtomicU64::new(0),
            length: AtomicU64::new(0),
            closed: AtomicBool::new(false),
        });
        let state = progress.clone();
        thread::Builder::new()
            .name("loom-file-reader".into())
            .spawn(move || {
                let mut source = Source::open(&path);
                let mut cached: Option<Arc<Page>> = None;
                while let Ok(request) = incoming.recv() {
                    if state.closed.load(Ordering::Relaxed) {
                        break;
                    }
                    if request.id != state.generation.load(Ordering::Relaxed) {
                        continue;
                    }
                    let result = match &mut source {
                        Ok(source) => {
                            execute(source, request.command, &state, request.id, cached.take())
                        }
                        Err(error) => Err(anyhow::anyhow!("{error:#}")),
                    };
                    if request.id != state.generation.load(Ordering::Relaxed) {
                        continue;
                    }
                    if let Ok(Some(page)) = &result {
                        cached = Some(page.clone());
                    }
                    let response = Response {
                        id: request.id,
                        result: result.map_err(|error| format!("{error:#}")),
                    };
                    if outgoing.send(response).is_err() {
                        break;
                    }
                }
            })?;
        Ok(Self {
            requests,
            results,
            progress,
            pending: None,
        })
    }

    pub fn request(&mut self, command: Command) -> u64 {
        let id = self.progress.generation.fetch_add(1, Ordering::Relaxed) + 1;
        self.pending = Some(Request { id, command });
        self.flush();
        id
    }

    fn flush(&mut self) {
        if let Some(request) = self.pending.take() {
            match self.requests.try_send(request) {
                Ok(()) => {}
                Err(TrySendError::Full(request)) => self.pending = Some(request),
                Err(TrySendError::Disconnected(_)) => {}
            }
        }
    }

    pub fn poll(&mut self) -> Option<Response> {
        self.flush();
        let mut latest = None;
        while let Ok(response) = self.results.try_recv() {
            if response.id == self.progress.generation.load(Ordering::Relaxed) {
                latest = Some(response);
            }
        }
        latest
    }

    pub fn cancel(&mut self) {
        self.progress.generation.fetch_add(1, Ordering::Relaxed);
        self.pending = None;
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.progress.closed.store(true, Ordering::Relaxed);
        self.cancel();
    }
}

fn execute(
    source: &mut Source,
    command: Command,
    progress: &Progress,
    id: u64,
    cached: Option<Arc<Page>>,
) -> Result<Option<Arc<Page>>> {
    source.unchanged()?;
    if let (Command::Page(view), Some(page)) = (&command, &cached) {
        let mut previous = page.view.clone();
        previous.selected = view.selected;
        if previous == *view
            && view.selected >= view.top
            && page
                .rows
                .last()
                .is_some_and(|row| view.selected <= row.number)
        {
            let mut page = (**page).clone();
            page.view = view.clone();
            page.notice = None;
            return Ok(Some(Arc::new(page)));
        }
    }
    // Release the cached text before reading its replacement (two screens at most).
    drop(cached);
    let mut view = match command {
        Command::Page(view) => view,
        Command::Last(mut view) => {
            view.selected = source.last_line(progress, id)?;
            view.top = view
                .selected
                .saturating_sub((view.height as u64).saturating_sub(1))
                .max(1);
            view
        }
        Command::Search {
            query,
            backwards,
            initial,
            mut view,
        } => {
            let (line, byte) = if initial {
                (view.selected, None)
            } else if let Some(hit) = &view.hit
                && hit.query == query
                && hit.line == view.selected
            {
                (
                    hit.line,
                    Some(if backwards {
                        hit.bytes.start
                    } else {
                        hit.bytes.start.saturating_add(1)
                    }),
                )
            } else {
                (view.selected, None)
            };
            let Some(hit) = source.find(&query, line, byte, backwards, progress, id)? else {
                return Ok(None);
            };
            view.selected = hit.line;
            view.top = hit.line.saturating_sub(view.height as u64 / 2).max(1);
            if hit.column < view.left
                || hit.end_column > view.left.saturating_add(view.width as u64)
            {
                view.left = hit.column;
            }
            view.hit = Some(hit);
            view
        }
    };
    view.height = view.height.max(1);
    view.width = view.width.max(1);
    Ok(Some(Arc::new(source.page(view, progress, id)?)))
}
