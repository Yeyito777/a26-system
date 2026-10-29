//! Rich content's state (WP-C1): image previews (`images.rs`), which code
//! blocks wrap, and the actions of what markdown draws (open an image,
//! wrap a code block). Opening and copying text go through `UiAct`.

use std::collections::HashSet;
use std::time::Instant;

use serde_json::Value;

use super::super::images::{ImageStore, Pic, Want};
use super::super::proto::Event;
use super::super::requests::Pending;
use super::Model;

#[derive(Default)]
pub struct Rich {
    pub images: ImageStore,
    /// Code blocks drawn wrapped (by their key) instead of scrolling
    /// sideways.
    pub wrapped: HashSet<u64>,
    wraps: u64,
}

impl Rich {
    /// Changes whenever anything here changes how a piece looks: part of
    /// the transcript's layout key.
    pub fn version(&self) -> u64 {
        self.images.version.wrapping_add(self.wraps << 32)
    }
}

#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)] // ToggleWrap: code blocks' header (next).
pub enum Act {
    /// The image viewer on a message's images, at `index`.
    ViewImages {
        pics: Vec<Pic>,
        index: usize,
        conv: Option<String>,
    },
    /// A code block wraps its lines, or stops wrapping them.
    ToggleWrap(u64),
    /// Asks again for a preview that failed (the viewer's Retry).
    RetryImage(Want),
}

impl Model {
    pub(super) fn handle_rich(&mut self, act: Act) -> Vec<Value> {
        match act {
            Act::ViewImages { pics, index, conv } => {
                self.ui.open(Box::new(super::super::viewer::Viewer::new(
                    pics, index, conv,
                )));
            }
            Act::ToggleWrap(key) => {
                if !self.rich.wrapped.remove(&key) {
                    self.rich.wrapped.insert(key);
                }
                self.rich.wraps += 1;
            }
            Act::RetryImage(want) => self.rich.images.retry(&want),
        }
        Vec::new()
    }

    /// The previews the screen shows or will soon, nearest first: asks for
    /// those not here yet.
    pub fn want_images(&mut self, wants: &[Want], now: Instant) -> Vec<Value> {
        if !self.connected {
            return Vec::new();
        }
        self.rich.images.want(wants, &mut self.requests, now)
    }

    /// Takes this area's events (by value: previews are big); gives the
    /// others back (moved, as `apply` would).
    #[allow(clippy::result_large_err)]
    pub(super) fn take_rich(&mut self, event: Event, _now: Instant) -> Result<Vec<Value>, Event> {
        match event {
            Event::ImageData(data) => {
                self.rich.images.arrived(data);
                Ok(Vec::new())
            }
            other => Err(other),
        }
    }

    /// An error answering one of this area's requests (a preview not
    /// found: asked again a little later).
    pub(super) fn rich_error(&mut self, id: &str, message: &str) -> bool {
        self.rich.images.failed(id, message, Instant::now());
        true
    }

    /// One of this area's requests was not answered in time.
    pub(super) fn rich_timeout(&mut self, id: &str, _pending: &Pending) -> bool {
        self.rich.images.failed(id, "timed out", Instant::now())
    }
}
