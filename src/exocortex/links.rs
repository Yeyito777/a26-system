//! Links in replies (WP-C1, PAR-140, 337, 338). A web address opens in
//! Moon's Browser; a mail address or number is copied; a local path is
//! resolved on the daemon's host (`resolve_file_link`: its path, kind and
//! size, never its content) and shown in a sheet with "Copy path".

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::Instant;

use serde_json::Value;

use crate::gfx::{Canvas, RectF};

use super::markdown::{LinkKind, link_kind};
use super::model::{Action, Effect, Model, UiAct};
use super::proto::{self, FileLink};
use super::requests::{self, Owner};
use super::theme::{self, T_BODY, T_META, TARGET};
use super::ui::icons::{self, Icon};
use super::ui::overlay::Cx;
use super::ui::sheet::{Reply, Sheet, SheetContent};
use super::ui::widgets::{self, Button, ButtonKind};

/// Where a local link is.
#[derive(Debug, Clone, PartialEq)]
pub enum Resolved {
    Asking,
    Found(FileLink),
    Failed(String),
}

/// Local links asked about, by target.
#[derive(Debug, Default)]
pub struct FileLinks {
    states: HashMap<(String, String), Resolved>,
    requests: HashMap<String, (String, String)>,
}

impl FileLinks {
    pub fn state(&self, conv: &str, target: &str) -> Option<&Resolved> {
        self.states.get(&(conv.to_owned(), target.to_owned()))
    }

    /// A `file_link_resolved`; whether it was asked for.
    pub fn resolved(&mut self, link: FileLink) -> bool {
        let Some(key) = link.req_id.as_ref().and_then(|id| self.requests.remove(id)) else {
            return false;
        };
        self.states.insert(key, Resolved::Found(link));
        true
    }

    /// An error (or no answer) for request `id`; whether it was ours.
    pub fn failed(&mut self, id: &str, message: &str) -> bool {
        let Some(key) = self.requests.remove(id) else {
            return false;
        };
        let message = message
            .strip_prefix("resolve_file_link: ")
            .unwrap_or(message)
            .to_owned();
        self.states.insert(key, Resolved::Failed(message));
        true
    }
}

impl Model {
    /// A tapped link.
    pub(super) fn open_link(&mut self, target: &str) -> Vec<Value> {
        match link_kind(target) {
            Some(LinkKind::Web(url)) => {
                if super::ui::openable(&url) {
                    self.ui.effect(Effect::OpenUrl(url));
                } else {
                    self.ui.toast("Only web links open in the Browser.");
                }
                Vec::new()
            }
            Some(LinkKind::Other(address)) => {
                let shown = address
                    .split_once(':')
                    .map_or(address.as_str(), |(_, rest)| rest)
                    .to_owned();
                self.ui.effect(Effect::Copy(shown.clone()));
                self.ui.haptic(super::ui::Haptic::Selection);
                self.ui.toast(format!("Copied {shown}"));
                Vec::new()
            }
            Some(LinkKind::File(_)) => {
                let Some(conv) = self.chat.id.clone() else {
                    return Vec::new();
                };
                self.ui.haptic(super::ui::Haptic::Light);
                self.ui.open(Box::new(Sheet::new(FileSheet {
                    conv: conv.clone(),
                    target: target.to_owned(),
                })));
                let key = (conv.clone(), target.to_owned());
                let links = &mut self.rich.links;
                if matches!(links.states.get(&key), Some(Resolved::Found(_))) {
                    return Vec::new();
                }
                if !self.connected {
                    links
                        .states
                        .insert(key, Resolved::Failed("Not connected to Exocortex.".into()));
                    return Vec::new();
                }
                let id = self.requests.issue(
                    Owner::Rich,
                    "file-link",
                    Some(&conv),
                    requests::LOAD_WITHIN,
                    Instant::now(),
                );
                let links = &mut self.rich.links;
                links.states.insert(key.clone(), Resolved::Asking);
                links.requests.insert(id.clone(), key);
                vec![proto::resolve_file_link(&id, &conv, target)]
            }
            None => Vec::new(),
        }
    }
}

/// The sheet of a local link: what it is on the daemon's host.
pub struct FileSheet {
    conv: String,
    target: String,
}

impl FileSheet {
    fn state<'a>(&self, cx: &'a Cx) -> Option<&'a Resolved> {
        cx.model.rich.links.state(&self.conv, &self.target)
    }

    fn lines(&self, text: &str, width: f32) -> Vec<String> {
        // Paths break anywhere, not at spaces only.
        let advance = crate::text::measure("M", T_BODY.weight, T_BODY.px()).max(1.0);
        let per_line = ((width / advance).floor() as usize).max(8);
        let chars: Vec<char> = text.chars().collect();
        chars
            .chunks(per_line)
            .map(|chunk| chunk.iter().collect())
            .collect()
    }

    fn button(&self, cx: &Cx, width: f32) -> Option<RectF> {
        let Some(Resolved::Found(link)) = self.state(cx) else {
            return None;
        };
        let top = self.found_top()
            + self.lines(&link.path, width).len() as f32 * T_BODY.line()
            + T_META.line()
            + 34.0;
        let w = Button::new("Copy path", ButtonKind::Secondary)
            .icon(Icon::Copy)
            .width();
        Some(RectF::new(0.0, top, w, TARGET))
    }

    fn target_h(&self) -> f32 {
        T_META.line() * 2.0
    }

    fn found_top(&self) -> f32 {
        self.target_h() + 22.0
    }
}

impl SheetContent for FileSheet {
    fn title(&self) -> String {
        "File link".into()
    }

    fn layout(&mut self, cx: &Cx, width: f32) -> f32 {
        match (self.state(cx), self.button(cx, width)) {
            (_, Some(button)) => button.y + button.h + 22.0,
            _ => self.found_top() + T_BODY.line() * 2.0 + 22.0,
        }
    }

    fn draw(&self, canvas: &mut Canvas, cx: &Cx, x: f32, top: f32, pressed: Option<usize>) {
        let p = theme::p();
        let width = cx.width - 2.0 * x;
        let target = widgets::fit(&self.target, T_META, width);
        widgets::text(
            canvas,
            &target,
            x,
            T_META.baseline(top),
            T_META,
            p.muted,
            1.0,
        );
        let y = top + self.found_top();
        match self.state(cx) {
            None | Some(Resolved::Asking) => {
                widgets::text(
                    canvas,
                    "Resolving on the daemon host…",
                    x,
                    T_BODY.baseline(y),
                    T_BODY,
                    p.muted,
                    1.0,
                );
            }
            Some(Resolved::Failed(message)) => {
                for (index, line) in widgets::wrap_plain(message, T_BODY, width)
                    .iter()
                    .take(2)
                    .enumerate()
                {
                    widgets::text(
                        canvas,
                        line,
                        x,
                        T_BODY.baseline(y + index as f32 * T_BODY.line()),
                        T_BODY,
                        p.error,
                        1.0,
                    );
                }
            }
            Some(Resolved::Found(link)) => {
                let directory = link.kind == "directory";
                icons::draw(
                    canvas,
                    if directory { Icon::Folder } else { Icon::File },
                    x + 22.0,
                    y + T_BODY.line() / 2.0,
                    44.0,
                    p.secondary_accent,
                    1.0,
                );
                let lines = self.lines(&link.path, width - 66.0);
                for (index, line) in lines.iter().enumerate() {
                    widgets::text(
                        canvas,
                        line,
                        x + 66.0,
                        T_BODY.baseline(y + index as f32 * T_BODY.line()),
                        T_BODY,
                        p.text,
                        1.0,
                    );
                }
                let meta = if directory {
                    "Directory · on the daemon host".to_owned()
                } else {
                    format!("File · {} · on the daemon host", bytes(link.size))
                };
                widgets::text(
                    canvas,
                    &meta,
                    x + 66.0,
                    T_META.baseline(y + lines.len() as f32 * T_BODY.line()),
                    T_META,
                    p.muted,
                    1.0,
                );
                if let Some(button) = self.button(cx, width) {
                    let rect = RectF::new(x + button.x, top + button.y, button.w, button.h);
                    let face = Button::new("Copy path", ButtonKind::Secondary)
                        .icon(Icon::Copy)
                        .pressed(pressed == Some(0));
                    widgets::button(canvas, rect, &face, p.sidebar_background);
                }
            }
        }
    }

    fn item_at(&self, cx: &Cx, x: f32, y: f32) -> Option<usize> {
        let width = cx.width - 2.0 * theme::ACTION_PAD;
        self.button(cx, width)
            .filter(|button| button.contains(x, y))
            .map(|_| 0)
    }

    fn activate(&mut self, cx: &Cx, _index: usize) -> Reply {
        match self.state(cx) {
            Some(Resolved::Found(link)) => {
                Reply::Action(Action::Ui(UiAct::Copy(link.path.clone())))
            }
            _ => Reply::None,
        }
    }

    fn key(&self, cx: &Cx) -> u64 {
        let mut hasher = DefaultHasher::new();
        (&self.target, format!("{:?}", self.state(cx))).hash(&mut hasher);
        hasher.finish()
    }

    fn scoped(&self) -> bool {
        true
    }
}

/// 482113 as "471 KB" (the iPhone's `formatBytes`).
fn bytes(size: u64) -> String {
    if size < 1024 {
        format!("{size} B")
    } else if size < 1 << 20 {
        format!("{} KB", (size as f64 / 1024.0).round())
    } else {
        format!("{:.1} MB", size as f64 / f64::from(1 << 20))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exocortex::model::rich::Act;
    use crate::exocortex::proto::{Event, parse_event};

    fn chat() -> Model {
        let mut model = Model::new();
        model.on_connected();
        model.handle(Action::Nav(crate::exocortex::model::Nav::OpenConversation(
            "c".into(),
        )));
        model.ui.effects.clear();
        model.ui.opening.clear();
        model
    }

    #[test]
    fn web_links_open_in_the_browser_others_are_copied() {
        let mut model = chat();
        model.handle(Action::Rich(Act::OpenLink("www.rust-lang.org".into())));
        assert_eq!(
            model.ui.effects,
            vec![Effect::OpenUrl("https://www.rust-lang.org".into())]
        );
        model.ui.effects.clear();
        model.handle(Action::Rich(Act::OpenLink("mailto:a@b.c".into())));
        assert_eq!(model.ui.effects[0], Effect::Copy("a@b.c".into()));
    }

    #[test]
    fn a_local_path_is_resolved_on_the_daemon_and_shown() {
        let mut model = chat();
        let commands = model.handle(Action::Rich(Act::OpenLink("./docs/notes.md".into())));
        assert_eq!(commands[0]["type"], "resolve_file_link");
        assert_eq!(commands[0]["convId"], "c");
        assert_eq!(commands[0]["target"], "./docs/notes.md");
        assert_eq!(model.ui.opening.len(), 1);
        assert_eq!(
            model.rich.links.state("c", "./docs/notes.md"),
            Some(&Resolved::Asking)
        );
        let id = commands[0]["reqId"].as_str().unwrap();
        model.apply(
            parse_event(&format!(
                r#"{{"type":"file_link_resolved","reqId":"{id}","convId":"c","path":"/root/docs/notes.md","kind":"file","size":2048}}"#
            )),
            Instant::now(),
        );
        assert!(matches!(
            model.rich.links.state("c", "./docs/notes.md"),
            Some(Resolved::Found(link)) if link.path == "/root/docs/notes.md"
        ));
        // Another one the daemon cannot find.
        let commands = model.handle(Action::Rich(Act::OpenLink("~/gone".into())));
        let id = commands[0]["reqId"].as_str().unwrap();
        model.apply(
            parse_event(&format!(
                r#"{{"type":"error","reqId":"{id}","message":"resolve_file_link: No such file"}}"#
            )),
            Instant::now(),
        );
        assert_eq!(
            model.rich.links.state("c", "~/gone"),
            Some(&Resolved::Failed("No such file".into()))
        );
        let _ = Event::Other;
        assert_eq!(bytes(2048), "2 KB");
    }
}
