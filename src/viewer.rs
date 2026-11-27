mod source;
mod worker;

use std::{
    path::PathBuf,
    sync::{Arc, atomic::Ordering},
};

pub(crate) use source::{Page, View};
use worker::{Client, Command};

#[derive(Debug)]
pub(crate) struct Viewer {
    pub path: PathBuf,
    pub page: Option<Arc<Page>>,
    pub message: Option<String>,
    pub busy: bool,
    pub invalidated: bool,
    pub query: String,
    pub viewport: (usize, usize),
    client: Client,
    request_id: u64,
}

impl Viewer {
    pub fn open(path: PathBuf) -> std::io::Result<Self> {
        let client = Client::start(path.clone())?;
        Ok(Self {
            path,
            page: None,
            message: None,
            busy: false,
            invalidated: false,
            query: String::new(),
            viewport: (0, 0),
            client,
            request_id: 0,
        })
    }

    pub fn view(&self) -> View {
        let mut view = self.page.as_ref().map_or(
            View {
                selected: 1,
                top: 1,
                left: 0,
                height: self.viewport.0,
                width: self.viewport.1,
                hit: None,
            },
            |page| page.view.clone(),
        );
        view.height = self.viewport.0.max(1);
        view.width = self.viewport.1.max(1);
        view
    }

    pub fn poll(&mut self) -> bool {
        if let Some(response) = self.client.poll() {
            if response.id != self.request_id {
                return false;
            }
            self.busy = false;
            match response.result {
                Ok(Some(page)) => {
                    self.message = page.notice.clone();
                    self.page = Some(page);
                }
                Ok(None) => self.message = Some("No match before the file boundary".into()),
                Err(error) => {
                    self.invalidated = true;
                    self.message = Some(format!("{error}. Close and reopen the file."));
                    return self.page.is_none();
                }
            }
        }
        false
    }

    fn send(&mut self, command: Command) {
        if self.invalidated {
            return;
        }
        self.message = None;
        self.busy = true;
        self.request_id = self.client.request(command);
    }

    pub fn load(&mut self, view: View) {
        self.send(Command::Page(view));
    }

    pub fn last(&mut self) {
        self.send(Command::Last(self.view()));
    }

    pub fn search(&mut self, backwards: bool, initial: bool) {
        if self.query.is_empty() {
            self.message = Some("Enter a search query with /".into());
            return;
        }
        self.send(Command::Search {
            query: self.query.clone(),
            backwards,
            initial,
            view: self.view(),
        });
    }

    pub fn cancel(&mut self) {
        self.client.cancel();
        self.busy = false;
        self.message = Some("Cancelled".into());
    }

    pub fn progress(&self) -> String {
        let offset = self.client.progress.offset.load(Ordering::Relaxed);
        let length = self.client.progress.length.load(Ordering::Relaxed);
        format!("Reading: {offset}/{length} bytes | Esc cancels")
    }
}
