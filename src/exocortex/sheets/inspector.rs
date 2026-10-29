//! The model sheet: the conversation's effort, then the models of every
//! signed-in provider.

use std::time::Instant;

use crate::gfx::{Canvas, Path, RectF};
use crate::text::{self, Weight};
use crate::ui::{
    BLUE, MARGIN, SECONDARY, SECONDARY_ALPHA, SEPARATOR, TERTIARY_ALPHA, WHITE, baseline,
};

use super::super::model::{Action, Model};
use super::super::view::{Hit, ViewState, W, pressed_action};

// Model sheet.
pub(crate) const SHEET_TOP: f32 = 300.0;
pub(crate) const SHEET_HEADER: f32 = 150.0;
pub(crate) const SHEET_ROW: f32 = 118.0;
pub(crate) const SHEET_SECTION: f32 = 84.0;
pub(crate) const CHIP_H: f32 = 84.0;

pub(crate) fn sheet_content(model: &Model) -> Vec<(f32, SheetItem)> {
    let mut items = Vec::new();
    let mut y = 0.0;
    let selection = model.selection();
    let efforts: Vec<String> = model
        .providers
        .iter()
        .find(|p| p.id == selection.provider)
        .and_then(|p| p.models.iter().find(|m| m.id == selection.model))
        .map(|m| {
            m.supported_efforts
                .iter()
                .map(|e| e.effort.clone())
                .collect()
        })
        .unwrap_or_default();
    if !efforts.is_empty() {
        items.push((y, SheetItem::Section("Effort".into())));
        y += SHEET_SECTION;
        items.push((y, SheetItem::Efforts(efforts)));
        y += CHIP_H + 40.0;
    }
    // The conversation's own provider first.
    let mut providers = model.usable_providers();
    providers.sort_by_key(|p| p.id != selection.provider);
    for provider in providers {
        items.push((y, SheetItem::Section(provider.label.clone())));
        y += SHEET_SECTION;
        for m in &provider.models {
            items.push((
                y,
                SheetItem::Model {
                    provider: provider.id.clone(),
                    id: m.id.clone(),
                    label: m.label.clone(),
                },
            ));
            y += SHEET_ROW;
        }
        y += 24.0;
    }
    items.push((y, SheetItem::End));
    items
}

#[derive(Debug, Clone)]
pub(crate) enum SheetItem {
    Section(String),
    Efforts(Vec<String>),
    Model {
        provider: String,
        id: String,
        label: String,
    },
    End,
}

pub fn sheet_max(model: &Model, height: f32) -> f32 {
    let content = sheet_content(model).last().map_or(0.0, |(y, _)| *y);
    let visible = height - SHEET_TOP - SHEET_HEADER;
    (content + 60.0 - visible).max(0.0)
}

pub(crate) fn effort_chips(efforts: &[String], top: f32) -> Vec<(RectF, String)> {
    let mut x = MARGIN + 10.0;
    let mut out = Vec::new();
    for effort in efforts {
        let w = text::measure(effort, Weight::Medium, 34.0) + 56.0;
        out.push((RectF::new(x, top, w, CHIP_H - 16.0), effort.clone()));
        x += w + 16.0;
    }
    // Too many for one line: shrink them evenly.
    let right = out.last().map_or(0.0, |(r, _)| r.x + r.w);
    if right > W - MARGIN {
        let scale = (W - 2.0 * MARGIN - 10.0) / (right - MARGIN);
        for (rect, _) in &mut out {
            rect.x = MARGIN + (rect.x - MARGIN) * scale;
            rect.w *= scale;
        }
    }
    out
}

pub(crate) fn sheet_rect(height: f32) -> RectF {
    RectF::new(0.0, SHEET_TOP, W, height - SHEET_TOP + 60.0)
}

pub(crate) fn draw_sheet(
    canvas: &mut Canvas,
    model: &Model,
    view: &mut ViewState,
    pressed: Option<&Hit>,
    height: f32,
    now: Instant,
) {
    canvas.fill_rect(0, 0, W as i32, height as i32, 0x000000, 0.55);
    let sheet = sheet_rect(height);
    canvas.fill_rrect(sheet, 40.0, 0x1c1c1e, 1.0);
    canvas.fill_rrect(
        RectF::new(W / 2.0 - 60.0, SHEET_TOP + 18.0, 120.0, 12.0),
        6.0,
        0x5a5a5e,
        1.0,
    );
    text::draw(
        canvas,
        "Model",
        MARGIN + 10.0,
        SHEET_TOP + 104.0,
        Weight::Bold,
        56.0,
        WHITE,
        1.0,
    );
    let pressed = pressed_action(pressed).cloned();
    let done_alpha = if pressed == Some(Action::HidePicker) {
        0.4
    } else {
        1.0
    };
    text::draw_right(
        canvas,
        "Done",
        W - MARGIN - 10.0,
        SHEET_TOP + 100.0,
        Weight::SemiBold,
        46.0,
        BLUE,
        done_alpha,
    );
    let top = SHEET_TOP + SHEET_HEADER;
    let bottom = height - 40.0;
    let offset = view.sheet_scroll.value(now);
    let selection = model.selection();
    canvas.set_clip(0, top as i32, W as i32, bottom as i32);
    for (y, item) in sheet_content(model) {
        let y = top + y - offset;
        if y > bottom || y + SHEET_ROW < top {
            continue;
        }
        match item {
            SheetItem::Section(title) => {
                text::draw(
                    canvas,
                    &title.to_uppercase(),
                    MARGIN + 10.0,
                    y + 58.0,
                    Weight::Regular,
                    34.0,
                    SECONDARY,
                    SECONDARY_ALPHA,
                );
            }
            SheetItem::Efforts(efforts) => {
                for (rect, effort) in effort_chips(&efforts, y) {
                    let rect = RectF::new(rect.x, rect.y, rect.w, rect.h);
                    let current = effort == selection.effort;
                    let down = pressed == Some(Action::PickEffort(effort.clone()));
                    canvas.fill_rrect(
                        rect,
                        rect.h / 2.0,
                        if current { BLUE } else { 0x2c2c2e },
                        if down { 0.6 } else { 1.0 },
                    );
                    text::draw_centered(
                        canvas,
                        &effort,
                        rect.x + rect.w / 2.0,
                        baseline(rect.y + rect.h / 2.0, Weight::Medium, 34.0),
                        Weight::Medium,
                        34.0,
                        WHITE,
                        1.0,
                    );
                }
            }
            SheetItem::Model {
                provider,
                id,
                label,
            } => {
                let rect = RectF::new(MARGIN, y, W - 2.0 * MARGIN, SHEET_ROW);
                if pressed == Some(Action::PickModel(provider.clone(), id.clone())) {
                    canvas.fill_rrect(rect, 20.0, 0x3a3a3c, 1.0);
                }
                text::draw(
                    canvas,
                    &label,
                    MARGIN + 30.0,
                    baseline(y + SHEET_ROW / 2.0, Weight::Regular, 46.0),
                    Weight::Regular,
                    46.0,
                    WHITE,
                    1.0,
                );
                let id_text = text::fit(&id, Weight::Regular, 32.0, 380.0);
                let current = selection.provider == provider && selection.model == id;
                text::draw_right(
                    canvas,
                    &id_text,
                    W - MARGIN - 90.0,
                    baseline(y + SHEET_ROW / 2.0, Weight::Regular, 32.0),
                    Weight::Regular,
                    32.0,
                    SECONDARY,
                    TERTIARY_ALPHA,
                );
                if current {
                    let mut check = Path::new();
                    let (cx, cy) = (W - MARGIN - 44.0, y + SHEET_ROW / 2.0);
                    check.stroke_polyline(
                        &[
                            (cx - 18.0, cy),
                            (cx - 5.0, cy + 14.0),
                            (cx + 18.0, cy - 14.0),
                        ],
                        7.0,
                    );
                    canvas.fill_path(&check, BLUE, 1.0);
                }
                canvas.fill_rect(
                    (MARGIN + 30.0) as i32,
                    (y + SHEET_ROW - 2.0) as i32,
                    (W - 2.0 * MARGIN - 30.0) as i32,
                    2,
                    SEPARATOR,
                    1.0,
                );
            }
            SheetItem::End => {}
        }
    }
    canvas.reset_clip();
}
