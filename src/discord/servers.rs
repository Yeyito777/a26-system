//! The Servers tab: the server rail on the left (the user's order and
//! folders, unread pills, mention badges) and the chosen server's channels
//! beside it, by category, with joined threads under their channels, as
//! Discord's mobile app lays them out.

#![allow(dead_code)]

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use crate::comms::Scroll;
use crate::exocortex::gesture::{Axis, Gesture};
use crate::exocortex::ui::icons::{self, Icon};
use crate::gfx::{Canvas, RectF};
use crate::social::media::Media;
use crate::text::{self, Weight};

use super::Action;
use super::chat::channel_icon;
use super::model::{Guild, Model};
use super::theme::{self, dp};
use super::ui;
use crate::social::shell::Damage;

#[derive(Debug, Clone)]
enum RailItem {
    Guild {
        index: usize,
        folder: Option<String>,
    },
    /// A folder: open (its servers follow) or closed (a tile of their icons).
    Folder {
        id: String,
        color: Option<u32>,
        guilds: Vec<usize>,
        open: bool,
    },
    Separator,
}

#[derive(Debug, Clone)]
enum Row {
    Category {
        id: String,
        name: String,
        collapsed: bool,
    },
    Channel {
        id: String,
        thread: bool,
    },
}

pub struct ServersView {
    pub selected: Option<String>,
    rail_scroll: Scroll,
    list_scroll: Scroll,
    open_folders: HashSet<String>,
    collapsed: HashSet<String>,
    rail: Vec<RailItem>,
    rail_key: (u64, usize),
    rows: Vec<Row>,
    rows_key: (u64, u64, Option<String>, usize),
    /// The channel last opened in each server (drawn selected).
    pub last_channel: HashMap<String, String>,
    dragging_rail: bool,
    /// What the rail and the list showed last frame (each is redrawn only when it changes).
    last_rail: Option<RailKey>,
    last_list: Option<ListKey>,
    /// Something besides scrolling changed: the next frame is whole.
    pub dirty: bool,
}

/// What the rail shows: servers, its offset, the chosen server, open folders, the height.
type RailKey = (u64, i32, Option<String>, usize, i32);
/// What the channel list shows: chats, servers, collapsed categories, its offset, server, channel, height.
type ListKey = (u64, u64, usize, i32, Option<String>, Option<String>, i32);

const ITEM: f32 = theme::SERVER_ICON + theme::SERVER_GAP;
/// What a missed picture redraws (`ui::set_tag`): the rail, the channel list.
const RAIL_TAG: u64 = 2;
const LIST_TAG: u64 = 4;

/// How a frame of the tab was drawn.
pub enum ServersFrame {
    /// Everything (the tab bar too).
    Whole,
    /// The channel list (and maybe the rail): the shell compares the frame.
    Compared,
    /// Only the rail's column: its damage, within the rail's columns.
    Rail(Damage),
    Unchanged,
}
const ROW_H: f32 = dp(38.0);
const CATEGORY_H: f32 = dp(40.0);
const LIST_HEADER: f32 = dp(64.0);

fn bottom(height: f32) -> f32 {
    height - theme::SAFE_BOTTOM - theme::TAB_H
}

fn acronym(name: &str) -> String {
    let mut out = String::new();
    for word in name.split_whitespace() {
        if let Some(c) = word.chars().next() {
            out.push(c);
        }
        if out.chars().count() >= 3 {
            break;
        }
    }
    if out.is_empty() { "?".to_owned() } else { out }
}

impl ServersView {
    pub fn new() -> Self {
        Self {
            selected: None,
            rail_scroll: Scroll::default(),
            list_scroll: Scroll::default(),
            open_folders: HashSet::new(),
            collapsed: HashSet::new(),
            rail: Vec::new(),
            rail_key: (u64::MAX, 0),
            rows: Vec::new(),
            rows_key: (u64::MAX, u64::MAX, None, 0),
            last_channel: HashMap::new(),
            dragging_rail: false,
            last_rail: None,
            last_list: None,
            dirty: true,
        }
    }

    pub fn guild<'a>(&self, model: &'a Model) -> Option<&'a Guild> {
        match &self.selected {
            Some(id) => model.guild(id).or_else(|| model.guilds.first()),
            None => model.guilds.first(),
        }
    }

    fn sync(&mut self, model: &Model) {
        let rail_key = (model.guilds_revision, self.open_folders.len());
        if rail_key != self.rail_key {
            self.rail_key = rail_key;
            self.rail.clear();
            let mut index = 0;
            while index < model.guilds.len() {
                let guild = &model.guilds[index];
                match &guild.folder {
                    Some(folder) => {
                        let id = folder.id.clone();
                        let mut members = Vec::new();
                        while index < model.guilds.len()
                            && model.guilds[index]
                                .folder
                                .as_ref()
                                .is_some_and(|f| f.id == id)
                        {
                            members.push(index);
                            index += 1;
                        }
                        let open = self.open_folders.contains(&id);
                        self.rail.push(RailItem::Folder {
                            id: id.clone(),
                            color: folder.color,
                            guilds: members.clone(),
                            open,
                        });
                        if open {
                            for member in members {
                                self.rail.push(RailItem::Guild {
                                    index: member,
                                    folder: Some(id.clone()),
                                });
                            }
                        }
                    }
                    None => {
                        self.rail.push(RailItem::Guild {
                            index,
                            folder: None,
                        });
                        index += 1;
                    }
                }
            }
        }
        let guild = self.guild(model).map(|g| g.id.clone());
        let rows_key = (
            model.chats_revision,
            model.guilds_revision,
            guild.clone(),
            self.collapsed.len(),
        );
        if rows_key == self.rows_key {
            return;
        }
        self.rows_key = rows_key;
        self.rows.clear();
        let Some(guild_id) = guild else { return };
        let Some(guild) = model.guild(&guild_id) else {
            return;
        };
        let mut by_category: HashMap<Option<String>, Vec<(&str, i64, bool)>> = HashMap::new();
        let mut threads: HashMap<&str, Vec<(&str, i64)>> = HashMap::new();
        for (id, chat) in &model.chats {
            let Some(ext) = model.ext(id) else { continue };
            if ext.guild.as_deref() != Some(guild_id.as_str()) {
                continue;
            }
            if chat.kind == "thread" {
                if let Some(parent) = chat.parent.as_deref() {
                    threads.entry(parent).or_default().push((id, chat.ts));
                }
                continue;
            }
            by_category.entry(ext.category.clone()).or_default().push((
                id,
                ext.position,
                chat.kind == "voice",
            ));
        }
        for children in threads.values_mut() {
            children.sort_by(|a, b| b.1.cmp(&a.1));
        }
        let selected = self.last_channel.get(&guild_id).cloned();
        let push_channels =
            |rows: &mut Vec<Row>, list: &mut Vec<(&str, i64, bool)>, collapsed: bool| {
                push_rows(rows, list, collapsed, model, &threads, selected.as_deref());
            };
        if let Some(mut list) = by_category.remove(&None) {
            push_channels(&mut self.rows, &mut list, false);
        }
        let mut categories = guild.categories.clone();
        categories.sort_by_key(|c| c.position);
        for category in categories {
            let Some(mut list) = by_category.remove(&Some(category.id.clone())) else {
                continue;
            };
            let collapsed = self.collapsed.contains(&category.id);
            self.rows.push(Row::Category {
                id: category.id.clone(),
                name: category.name.to_uppercase(),
                collapsed,
            });
            push_channels(&mut self.rows, &mut list, collapsed);
        }
        // Channels in categories we were not told about.
        let mut rest: Vec<_> = by_category.into_values().flatten().collect();
        if !rest.is_empty() {
            push_channels(&mut self.rows, &mut rest, false);
        }
    }

    fn rail_height(&self) -> f32 {
        self.rail.len() as f32 * ITEM + dp(16.0)
    }

    fn list_height(&self) -> f32 {
        LIST_HEADER
            + self
                .rows
                .iter()
                .map(|row| match row {
                    Row::Category { .. } => CATEGORY_H,
                    Row::Channel { thread: true, .. } => ROW_H - dp(4.0),
                    Row::Channel { .. } => ROW_H,
                })
                .sum::<f32>()
            + dp(24.0)
    }

    fn rail_max(&self, height: f32) -> f32 {
        (self.rail_height() - (bottom(height) - theme::TOP)).max(0.0)
    }

    fn list_max(&self, height: f32) -> f32 {
        (self.list_height() - (bottom(height) - theme::TOP)).max(0.0)
    }

    pub fn tick(&mut self, model: &Model, height: f32, now: Instant) -> bool {
        self.sync(model);
        let a = self.rail_scroll.step(self.rail_max(height), now);
        let b = self.list_scroll.step(self.list_max(height), now);
        a || b
    }

    pub fn gesture(
        &mut self,
        gesture: Gesture,
        model: &Model,
        width: f32,
        height: f32,
        now: Instant,
    ) -> Vec<Action> {
        self.sync(model);
        let mut actions = Vec::new();
        match gesture {
            Gesture::Tap { x, y } => {
                if y < theme::TOP || y > bottom(height) {
                    return actions;
                }
                if x < theme::RAIL_W {
                    let offset = self.rail_scroll.value(now);
                    let index = ((y - theme::TOP - dp(8.0) + offset) / ITEM).floor();
                    if index >= 0.0
                        && let Some(item) = self.rail.get(index as usize).cloned()
                    {
                        match item {
                            RailItem::Guild { index, .. } => {
                                let id = model.guilds[index].id.clone();
                                if self.selected.as_deref() != Some(id.as_str()) {
                                    self.selected = Some(id);
                                    self.list_scroll.reset();
                                    actions.push(Action::Haptic);
                                }
                            }
                            RailItem::Folder { id, .. } => {
                                if !self.open_folders.remove(&id) {
                                    self.open_folders.insert(id);
                                }
                            }
                            RailItem::Separator => {}
                        }
                        actions.push(Action::Redraw);
                    }
                    return actions;
                }
                let offset = self.list_scroll.value(now);
                let mut row_y = theme::TOP + LIST_HEADER - offset;
                let _ = width;
                for row in self.rows.clone() {
                    let h = match &row {
                        Row::Category { .. } => CATEGORY_H,
                        Row::Channel { thread: true, .. } => ROW_H - dp(4.0),
                        Row::Channel { .. } => ROW_H,
                    };
                    if y >= row_y && y < row_y + h {
                        match row {
                            Row::Category { id, .. } => {
                                if !self.collapsed.remove(&id) {
                                    self.collapsed.insert(id);
                                }
                                actions.push(Action::Redraw);
                            }
                            Row::Channel { id, .. } => {
                                let kind = model
                                    .chats
                                    .get(&id)
                                    .map(|c| c.kind.clone())
                                    .unwrap_or_default();
                                match kind.as_str() {
                                    "voice" => actions.push(Action::Toast(
                                        "Voice channels can't be joined from this app.".into(),
                                    )),
                                    "forum" => actions.push(Action::OpenForum(id)),
                                    _ => {
                                        if let Some(guild) = self.guild(model) {
                                            self.last_channel.insert(guild.id.clone(), id.clone());
                                            self.rows_key.0 = u64::MAX;
                                        }
                                        actions.push(Action::OpenChat(id));
                                    }
                                }
                            }
                        }
                        break;
                    }
                    row_y += h;
                }
            }
            Gesture::DragStart {
                axis: Axis::Vertical,
                x,
                y,
                start_y,
                start_x,
                ..
            } => {
                self.dragging_rail = start_x < theme::RAIL_W;
                let _ = x;
                let max = if self.dragging_rail {
                    self.rail_max(height)
                } else {
                    self.list_max(height)
                };
                let scroll = if self.dragging_rail {
                    &mut self.rail_scroll
                } else {
                    &mut self.list_scroll
                };
                scroll.begin(start_y, now);
                scroll.moved(y, max, now);
                actions.push(Action::Redraw);
            }
            Gesture::Drag {
                axis: Axis::Vertical,
                y,
                ..
            } => {
                let max = if self.dragging_rail {
                    self.rail_max(height)
                } else {
                    self.list_max(height)
                };
                let scroll = if self.dragging_rail {
                    &mut self.rail_scroll
                } else {
                    &mut self.list_scroll
                };
                scroll.moved(y, max, now);
                actions.push(Action::Redraw);
            }
            Gesture::DragEnd {
                axis: Axis::Vertical,
                ..
            } => {
                let max = if self.dragging_rail {
                    self.rail_max(height)
                } else {
                    self.list_max(height)
                };
                let scroll = if self.dragging_rail {
                    &mut self.rail_scroll
                } else {
                    &mut self.list_scroll
                };
                scroll.end(max, now);
                actions.push(Action::Animate);
            }
            _ => {}
        }
        actions
    }

    /// Draws the tab. The rail and the channel list are each drawn only
    /// when something in it changed (the canvas keeps the last frame).
    pub fn draw(
        &mut self,
        canvas: &mut Canvas,
        model: &Model,
        media: &mut Media,
        now: Instant,
        full: bool,
    ) -> ServersFrame {
        self.sync(model);
        let (width, height) = (canvas.width as f32, canvas.height as f32);
        let bottom = bottom(height);
        let selected = self.guild(model).map(|g| g.id.clone());
        let rail_offset = self.rail_scroll.value(now).round();
        let list_offset = self.list_scroll.value(now).round();
        let rail_key = (
            model.guilds_revision,
            rail_offset as i32,
            selected.clone(),
            self.open_folders.len(),
            canvas.height,
        );
        let channel = selected
            .as_ref()
            .and_then(|g| self.last_channel.get(g).cloned());
        let list_key = (
            self.rows_key.0,
            self.rows_key.1,
            self.rows_key.3,
            list_offset as i32,
            selected.clone(),
            channel,
            canvas.height,
        );
        let ready = ui::take_ready(media);
        let whole = full || self.dirty || self.last_rail.is_none() || ready.contains(&0);
        let rail_x = theme::RAIL_W as i32;
        let (top_i, bottom_i) = (theme::TOP as i32, bottom as i32);
        if whole {
            ui::clear_misses();
            self.dirty = false;
            self.last_rail = Some(rail_key);
            self.last_list = Some(list_key);
            self.draw_rail(
                canvas,
                model,
                media,
                now,
                rail_offset,
                bottom,
                (0, bottom_i),
            );
            self.draw_list(canvas, model, now, list_offset, width, height, bottom);
            return ServersFrame::Whole;
        }
        let rail_changed = self.last_rail.as_ref() != Some(&rail_key) || ready.contains(&RAIL_TAG);
        let list_changed = self.last_list.as_ref() != Some(&list_key) || ready.contains(&LIST_TAG);
        let last_rail = self.last_rail.replace(rail_key.clone());
        self.last_list = Some(list_key);
        if list_changed {
            // The list (taps, its own scroll): drawn again, compared on the way out.
            if rail_changed {
                self.draw_rail(
                    canvas,
                    model,
                    media,
                    now,
                    rail_offset,
                    bottom,
                    (0, bottom_i),
                );
            }
            self.draw_list(canvas, model, now, list_offset, width, height, bottom);
            return ServersFrame::Compared;
        }
        if !rail_changed {
            return ServersFrame::Unchanged;
        }
        // Only the rail changed: it moves within its own column, and only
        // what came into view (or the whole column, for a picture) is drawn.
        let scrolled_only = last_rail.as_ref().is_some_and(|last| {
            (last.0, &last.2, last.3, last.4) == (rail_key.0, &rail_key.2, rail_key.3, rail_key.4)
        }) && !ready.contains(&RAIL_TAG);
        let dy = last_rail.as_ref().map_or(0, |last| last.1 - rail_key.1);
        if scrolled_only && dy.abs() < (bottom_i - top_i) / 2 {
            if dy == 0 {
                return ServersFrame::Unchanged;
            }
            ui::move_columns(canvas, 0, rail_x, top_i, bottom_i, dy);
            let band = if dy > 0 {
                (top_i, top_i + dy)
            } else {
                (bottom_i + dy, bottom_i)
            };
            self.draw_rail(canvas, model, media, now, rail_offset, bottom, band);
            return ServersFrame::Rail(Damage {
                moved: Some((top_i, bottom_i, dy)),
                rows: vec![band],
            });
        }
        self.draw_rail(
            canvas,
            model,
            media,
            now,
            rail_offset,
            bottom,
            (0, bottom_i),
        );
        ServersFrame::Rail(Damage {
            moved: None,
            rows: vec![(0, bottom_i)],
        })
    }

    /// Draws the rail's rows `band` (its column only).
    #[allow(clippy::too_many_arguments)]
    fn draw_rail(
        &self,
        canvas: &mut Canvas,
        model: &Model,
        media: &mut Media,
        now: Instant,
        offset: f32,
        bottom: f32,
        band: (i32, i32),
    ) {
        let (y0, y1) = (band.0.max(0), band.1.min(bottom as i32));
        if y1 <= y0 {
            return;
        }
        canvas.fill_rect(0, y0, theme::RAIL_W as i32, y1 - y0, theme::RAIL, 1.0);
        canvas.set_clip(0, y0.max(theme::TOP as i32), theme::RAIL_W as i32, y1);
        ui::set_tag(RAIL_TAG);
        let selected = self.guild(model).map(|g| g.id.clone());
        let cx = theme::RAIL_W / 2.0;
        for (i, item) in self.rail.iter().enumerate() {
            let top = theme::TOP + dp(8.0) + i as f32 * ITEM - offset;
            if top > bottom || top + ITEM < theme::TOP {
                continue;
            }
            let size = theme::SERVER_ICON;
            let icon = RectF {
                x: cx - size / 2.0,
                y: top,
                w: size,
                h: size,
            };
            match item {
                RailItem::Guild { index, folder } => {
                    let guild = &model.guilds[*index];
                    if folder.is_some() {
                        // A folder's servers sit on its tint.
                        let tint = guild
                            .folder
                            .as_ref()
                            .and_then(|f| f.color)
                            .unwrap_or(theme::BLURPLE);
                        canvas.fill_rect(
                            (cx - size / 2.0 - dp(4.0)) as i32,
                            (top - dp(4.0)) as i32,
                            (size + dp(8.0)) as i32,
                            ITEM as i32,
                            ui::shade(tint, theme::RAIL, 0.25),
                            1.0,
                        );
                    }
                    let is_selected = selected.as_deref() == Some(guild.id.as_str());
                    draw_guild_icon(canvas, guild, icon, is_selected, media, now);
                    // The pill: selected, else unread.
                    let pill_h = if is_selected {
                        dp(40.0)
                    } else if guild.unread {
                        dp(8.0)
                    } else {
                        0.0
                    };
                    if pill_h > 0.0 {
                        canvas.fill_rrect(
                            RectF {
                                x: -dp(4.0),
                                y: icon.y + (size - pill_h) / 2.0,
                                w: dp(8.0),
                                h: pill_h,
                            },
                            dp(4.0),
                            theme::WHITE,
                            1.0,
                        );
                    }
                    if guild.mentions > 0 {
                        ui::badge(
                            canvas,
                            guild.mentions,
                            icon.x + icon.w - dp(4.0),
                            icon.y + icon.h - dp(4.0),
                            Some(theme::RAIL),
                        );
                    }
                }
                RailItem::Folder {
                    color,
                    guilds,
                    open,
                    ..
                } => {
                    let tint = color.unwrap_or(theme::BLURPLE);
                    if *open {
                        canvas.fill_rrect(
                            RectF {
                                x: icon.x - dp(4.0),
                                y: icon.y - dp(4.0),
                                w: size + dp(8.0),
                                h: ITEM + dp(4.0),
                            },
                            dp(16.0),
                            ui::shade(tint, theme::RAIL, 0.25),
                            1.0,
                        );
                        icons::draw(
                            canvas,
                            Icon::FolderFill,
                            cx,
                            icon.y + size / 2.0,
                            dp(26.0),
                            tint,
                            1.0,
                        );
                    } else {
                        canvas.fill_rrect(icon, dp(16.0), ui::shade(tint, theme::RAIL, 0.35), 1.0);
                        let mini = dp(18.0);
                        for (n, index) in guilds.iter().take(4).enumerate() {
                            let mx = icon.x + dp(5.0) + (n % 2) as f32 * (mini + dp(2.0));
                            let my = icon.y + dp(5.0) + (n / 2) as f32 * (mini + dp(2.0));
                            let guild = &model.guilds[*index];
                            draw_guild_icon(
                                canvas,
                                guild,
                                RectF {
                                    x: mx,
                                    y: my,
                                    w: mini,
                                    h: mini,
                                },
                                false,
                                media,
                                now,
                            );
                        }
                        let unread = guilds.iter().any(|i| model.guilds[*i].unread);
                        let mentions: u32 = guilds.iter().map(|i| model.guilds[*i].mentions).sum();
                        if unread {
                            canvas.fill_rrect(
                                RectF {
                                    x: -dp(4.0),
                                    y: icon.y + (size - dp(8.0)) / 2.0,
                                    w: dp(8.0),
                                    h: dp(8.0),
                                },
                                dp(4.0),
                                theme::WHITE,
                                1.0,
                            );
                        }
                        if mentions > 0 {
                            ui::badge(
                                canvas,
                                mentions,
                                icon.x + icon.w - dp(4.0),
                                icon.y + icon.h - dp(4.0),
                                Some(theme::RAIL),
                            );
                        }
                    }
                }
                RailItem::Separator => {
                    canvas.fill_rect(
                        (cx - dp(16.0)) as i32,
                        (top + ITEM / 2.0) as i32,
                        dp(32.0) as i32,
                        2,
                        theme::DIVIDER,
                        1.0,
                    );
                }
            }
        }
        canvas.reset_clip();
        ui::set_tag(0);
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_list(
        &self,
        canvas: &mut Canvas,
        model: &Model,
        now: Instant,
        offset: f32,
        width: f32,
        height: f32,
        bottom: f32,
    ) {
        let _ = now;
        ui::set_tag(LIST_TAG);
        // Behind the panel's rounded corner.
        canvas.fill_rect(
            theme::RAIL_W as i32,
            0,
            (width - theme::RAIL_W) as i32,
            (theme::TOP + dp(20.0)) as i32,
            theme::RAIL,
            1.0,
        );
        let _ = height;
        // Down to the tab bar, which is drawn on its own.
        let panel = RectF {
            x: theme::RAIL_W,
            y: theme::TOP,
            w: width - theme::RAIL_W,
            h: bottom - theme::TOP,
        };
        // Only the top strip needs the rounded corner; the rest is a plain fill.
        canvas.fill_rrect(
            RectF {
                h: dp(40.0),
                ..panel
            },
            dp(16.0),
            theme::LIST,
            1.0,
        );
        canvas.fill_rect(
            panel.x as i32,
            (panel.y + dp(20.0)) as i32,
            panel.w as i32,
            (panel.h - dp(20.0)) as i32,
            theme::LIST,
            1.0,
        );
        let Some(guild) = self.guild(model) else {
            let note = if model.status.state == "online" {
                "No servers"
            } else {
                "Loading…"
            };
            text::draw_centered(
                canvas,
                note,
                panel.x + panel.w / 2.0,
                height / 2.0,
                Weight::Medium,
                theme::T_SMALL,
                theme::MUTED,
                1.0,
            );
            return;
        };
        let guild = guild.clone();
        canvas.set_clip(panel.x as i32, panel.y as i32, width as i32, bottom as i32);
        let x0 = panel.x;
        let header_y = theme::TOP - offset;
        let title = ui::fit(
            &guild.name,
            Weight::Bold,
            theme::T_TITLE,
            panel.w - dp(56.0),
        );
        let tw = text::draw(
            canvas,
            &title,
            x0 + dp(16.0),
            header_y + dp(34.0),
            Weight::Bold,
            theme::T_TITLE,
            theme::HEADER,
            1.0,
        );
        icons::draw(
            canvas,
            Icon::ChevronRight,
            x0 + dp(16.0) + tw + dp(12.0),
            header_y + dp(28.0),
            dp(16.0),
            theme::MUTED,
            1.0,
        );
        if let Some(members) = guild.members {
            canvas.fill_circle(
                x0 + dp(20.0),
                header_y + dp(50.0),
                dp(3.5),
                theme::MUTED,
                1.0,
            );
            text::draw(
                canvas,
                &format!("{members} Members"),
                x0 + dp(28.0),
                header_y + dp(54.0),
                Weight::Regular,
                theme::T_TINY,
                theme::MUTED,
                1.0,
            );
        }
        let mut y = theme::TOP + LIST_HEADER - offset;
        let selected_channel = self.last_channel.get(&guild.id).cloned();
        for row in &self.rows {
            let h = match row {
                Row::Category { .. } => CATEGORY_H,
                Row::Channel { thread: true, .. } => ROW_H - dp(4.0),
                Row::Channel { .. } => ROW_H,
            };
            if y > bottom {
                break;
            }
            if y + h < theme::TOP {
                y += h;
                continue;
            }
            match row {
                Row::Category {
                    name, collapsed, ..
                } => {
                    let cy = y + h / 2.0 + dp(5.0);
                    let icon = if *collapsed {
                        Icon::ChevronRight
                    } else {
                        Icon::ChevronDown
                    };
                    icons::draw(canvas, icon, x0 + dp(14.0), cy, dp(12.0), theme::MUTED, 1.0);
                    let label = ui::fit(name, Weight::SemiBold, theme::T_TINY, panel.w - dp(48.0));
                    text::draw_tracked(
                        canvas,
                        &label,
                        x0 + dp(24.0),
                        text::centered_baseline(cy, Weight::SemiBold, theme::T_TINY),
                        Weight::SemiBold,
                        theme::T_TINY,
                        0.6,
                        theme::MUTED,
                        1.0,
                    );
                }
                Row::Channel { id, thread } => {
                    let Some(chat) = model.chats.get(id) else {
                        y += h;
                        continue;
                    };
                    let ext = model.ext(id);
                    let cy = y + h / 2.0;
                    let is_selected = selected_channel.as_deref() == Some(id.as_str());
                    let unread = chat.unread > 0 && !chat.muted;
                    let indent = if *thread { dp(30.0) } else { 0.0 };
                    if is_selected {
                        canvas.fill_rrect(
                            RectF {
                                x: x0 + dp(8.0) + indent,
                                y: y + dp(1.0),
                                w: panel.w - dp(16.0) - indent,
                                h: h - dp(2.0),
                            },
                            dp(6.0),
                            theme::SELECTED,
                            1.0,
                        );
                    }
                    if unread && !is_selected {
                        canvas.fill_rrect(
                            RectF {
                                x: x0 - dp(4.0),
                                y: cy - dp(4.0),
                                w: dp(8.0),
                                h: dp(8.0),
                            },
                            dp(4.0),
                            theme::WHITE,
                            1.0,
                        );
                    }
                    let color = if is_selected || unread {
                        theme::HEADER
                    } else if chat.muted {
                        theme::DIM
                    } else {
                        theme::MUTED
                    };
                    let mut tx = x0 + dp(16.0) + indent;
                    if *thread {
                        ui::glyph(
                            canvas,
                            ui::Glyph::Spine,
                            x0 + dp(26.0),
                            cy - dp(6.0),
                            dp(20.0),
                            theme::DIM,
                            1.0,
                        );
                    } else {
                        channel_icon(
                            canvas,
                            &chat.kind,
                            ext.map_or(0, |e| e.channel_type),
                            tx + dp(10.0),
                            cy,
                            dp(20.0),
                            if chat.muted { theme::DIM } else { theme::MUTED },
                        );
                        tx += dp(28.0);
                    }
                    let size = if *thread {
                        theme::T_SMALL
                    } else {
                        theme::T_BODY
                    };
                    let weight = if unread || is_selected {
                        Weight::SemiBold
                    } else {
                        Weight::Medium
                    };
                    let badge_room = if chat.mentions > 0 { dp(36.0) } else { 0.0 };
                    let name = ui::fit(
                        &chat.title,
                        weight,
                        size,
                        x0 + panel.w - tx - dp(16.0) - badge_room,
                    );
                    text::draw(
                        canvas,
                        &name,
                        tx,
                        text::centered_baseline(cy, weight, size),
                        weight,
                        size,
                        color,
                        1.0,
                    );
                    if chat.mentions > 0 {
                        ui::badge(canvas, chat.mentions, x0 + panel.w - dp(26.0), cy, None);
                    }
                }
            }
            y += h;
        }
        canvas.reset_clip();
        ui::set_tag(0);
    }
}

/// A category's channels (text-like by position, then voice by position), each with its threads.
fn push_rows(
    rows: &mut Vec<Row>,
    list: &mut [(&str, i64, bool)],
    collapsed: bool,
    model: &Model,
    threads: &HashMap<&str, Vec<(&str, i64)>>,
    selected: Option<&str>,
) {
    list.sort_by(|a, b| a.2.cmp(&b.2).then(a.1.cmp(&b.1)).then(a.0.cmp(b.0)));
    for (id, _, _) in list.iter() {
        let chat = &model.chats[*id];
        let keep = !collapsed
            || (chat.unread > 0 && !chat.muted)
            || chat.mentions > 0
            || selected == Some(*id);
        if keep {
            rows.push(Row::Channel {
                id: (*id).to_owned(),
                thread: false,
            });
        }
        for (thread, _) in threads.get(id).map(Vec::as_slice).unwrap_or(&[]) {
            let t = &model.chats[*thread];
            if !collapsed || t.unread > 0 || t.mentions > 0 || selected == Some(*thread) {
                rows.push(Row::Channel {
                    id: (*thread).to_owned(),
                    thread: true,
                });
            }
        }
    }
}

impl Default for ServersView {
    fn default() -> Self {
        Self::new()
    }
}

pub fn draw_guild_icon(
    canvas: &mut Canvas,
    guild: &Guild,
    rect: RectF,
    selected: bool,
    media: &mut Media,
    now: Instant,
) {
    let radius = if selected {
        rect.w * 0.33
    } else {
        rect.w / 2.0
    };
    let image = guild
        .icon
        .as_ref()
        .and_then(|icon| ui::shaped(media, icon, rect.w, radius, now));
    match image {
        Some(image) => canvas.draw_image(&image, rect.x.round() as i32, rect.y.round() as i32, 1.0),
        None => {
            canvas.fill_rrect(
                rect,
                radius,
                if selected {
                    theme::BLURPLE
                } else {
                    theme::CHAT
                },
                1.0,
            );
            let label = acronym(&guild.name);
            let size = if rect.w < dp(30.0) {
                rect.w * 0.42
            } else if label.chars().count() > 2 {
                dp(13.0)
            } else {
                dp(16.0)
            };
            text::draw_centered(
                canvas,
                &label,
                rect.x + rect.w / 2.0,
                text::centered_baseline(rect.y + rect.h / 2.0, Weight::Medium, size),
                Weight::Medium,
                size,
                theme::HEADER,
                1.0,
            );
        }
    }
}
