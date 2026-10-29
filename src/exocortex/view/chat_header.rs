//! The chat's top bar: back, the conversation's title and its model.

use crate::gfx::{Canvas, Path};
use crate::text::{self, Weight};
use crate::ui::{BG, BLUE, MARGIN, SEPARATOR, WHITE, back_chevron, baseline};

use super::super::model::{Action, Model};
use super::{NAV_H, W};

/// The chat's navigation bar: back, title, model.
pub(crate) fn draw_nav(canvas: &mut Canvas, model: &Model, pressed_action: Option<&Action>) {
    canvas.fill_rect(0, 0, W as i32, NAV_H as i32, BG, 1.0);
    let back_alpha = if pressed_action == Some(&Action::Back) {
        0.4
    } else {
        1.0
    };
    back_chevron(canvas, MARGIN + 6.0, 72.0);
    text::draw(
        canvas,
        "Chats",
        MARGIN + 36.0,
        baseline(72.0, Weight::Regular, 47.0),
        Weight::Regular,
        47.0,
        BLUE,
        back_alpha,
    );
    let title = model
        .open_summary()
        .map_or("New Conversation", |s| s.display_title())
        .to_owned();
    let title = text::fit(&title, Weight::SemiBold, 44.0, 560.0);
    text::draw_centered(
        canvas,
        &title,
        W / 2.0 + 40.0,
        66.0,
        Weight::SemiBold,
        44.0,
        WHITE,
        1.0,
    );
    let selection = model.selection();
    let mut label = model.model_label(&selection);
    if !selection.effort.is_empty() {
        label = format!("{label} · {}", selection.effort);
    }
    let picker_alpha = if pressed_action == Some(&Action::ShowPicker) {
        0.4
    } else {
        1.0
    };
    let label = text::fit(&label, Weight::Medium, 34.0, 520.0);
    let label_w = text::measure(&label, Weight::Medium, 34.0);
    let lx = W / 2.0 + 40.0 - (label_w + 30.0) / 2.0;
    text::draw(
        canvas,
        &label,
        lx,
        116.0,
        Weight::Medium,
        34.0,
        BLUE,
        picker_alpha,
    );
    let mut caret = Path::new();
    let cx = lx + label_w + 20.0;
    caret.stroke_polyline(&[(cx - 9.0, 100.0), (cx, 109.0), (cx + 9.0, 100.0)], 4.5);
    canvas.fill_path(&caret, BLUE, picker_alpha);
    canvas.fill_rect(0, NAV_H as i32 - 2, W as i32, 2, SEPARATOR, 1.0);
}
