//! A new chat in a folder (PAR-085): the folder's instructions, which the
//! conversation will start with (its `effectiveInstructions`: the AGENTS.md
//! documents from the root folder down), as an "Instructions (inherited)"
//! box at the top of the draft, in the System Instructions box's look
//! (roadmap §3.6.8). A tap opens them in the reader. The TUI shows them the
//! same way in a blank chat in a folder.

use crate::gfx::{Canvas, RectF};

use super::super::model::{Action, Model, UiAct};
use super::super::theme::{self, HAIRLINE, T_EYEBROW, T_SMALL, TARGET, TRANSCRIPT_PAD, W};
use super::super::ui::icons::{self, Icon};
use super::super::ui::widgets;

const TITLE: &str = "Instructions (inherited)";
/// The reader's title (the box's does not fit its bar).
const READER_TITLE: &str = "Instructions";

/// The instructions a draft in the folder the list shows inherits, if any.
pub fn inherited(model: &Model) -> Option<&str> {
    let folder = model.current_folder()?;
    let text = model
        .sidebar
        .folders
        .get(folder)?
        .effective_instructions
        .as_deref()?;
    (!text.trim().is_empty()).then_some(text)
}

/// Where the box is, under the transcript's top.
pub fn rect(top: f32) -> RectF {
    RectF::new(
        TRANSCRIPT_PAD,
        top + theme::TRANSCRIPT_TOP,
        W - 2.0 * TRANSCRIPT_PAD,
        TARGET,
    )
}

/// The box's action (the reader on the instructions).
pub fn hit(model: &Model, top: f32, x: f32, y: f32) -> Option<Action> {
    let text = inherited(model)?;
    rect(top).contains(x, y).then(|| action(text))
}

fn action(text: &str) -> Action {
    Action::Ui(UiAct::Read {
        title: READER_TITLE.into(),
        text: text.to_owned(),
    })
}

/// Draws the box, if the draft inherits instructions.
pub fn draw(canvas: &mut Canvas, model: &Model, top: f32, pressed: Option<&Action>) {
    let Some(text) = inherited(model) else {
        return;
    };
    let p = theme::p();
    let c = theme::c();
    let r = rect(top);
    if pressed == Some(&action(text)) {
        widgets::fill(canvas, r, c.row_pressed);
    }
    widgets::border(canvas, r, HAIRLINE, c.instructions_border);
    let cy = r.y + r.h / 2.0;
    icons::draw(
        canvas,
        Icon::ChevronRight,
        r.x + 40.0,
        cy,
        40.0,
        p.muted,
        1.0,
    );
    let label_x = r.x + 80.0;
    let label_w = widgets::text(
        canvas,
        TITLE,
        label_x,
        T_EYEBROW.baseline(cy - T_EYEBROW.line() / 2.0),
        T_EYEBROW,
        p.accent,
        1.0,
    );
    // A line of it: the first that is not a heading.
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .unwrap_or_default();
    let room = r.x + r.w - 28.0 - (label_x + label_w + 22.0);
    if room > 60.0 {
        let preview = widgets::fit(line, T_SMALL, room);
        widgets::text(
            canvas,
            &preview,
            label_x + label_w + 22.0,
            T_SMALL.baseline(cy - T_SMALL.line() / 2.0),
            T_SMALL,
            p.muted,
            1.0,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exocortex::model::Nav;
    use crate::exocortex::proto::parse_event;
    use std::time::Instant;

    #[test]
    fn a_draft_in_a_folder_shows_what_it_inherits() {
        let mut model = Model::new();
        model.on_connected();
        model.apply(
            parse_event(r##"{"type":"conversations_list","conversations":[],"folders":[{"id":"f","name":"Project","parentId":null,"pinned":false,"sortOrder":0,"effectiveInstructions":"# Context from AGENTS.md:\nRemember dinosaurs."},{"id":"g","name":"Plain","parentId":null,"pinned":false,"sortOrder":1}]}"##),
            Instant::now(),
        );
        model.handle(Action::Nav(Nav::NewConversation));
        assert_eq!(inherited(&model), None, "outside folders: nothing");
        model.sidebar.draft_folder = Some("f".into());
        assert_eq!(
            inherited(&model),
            Some("# Context from AGENTS.md:\nRemember dinosaurs.")
        );
        let r = rect(124.0);
        assert_eq!(
            hit(&model, 124.0, r.x + 10.0, r.y + 10.0),
            Some(Action::Ui(UiAct::Read {
                title: READER_TITLE.into(),
                text: "# Context from AGENTS.md:\nRemember dinosaurs.".into()
            }))
        );
        model.sidebar.draft_folder = Some("g".into());
        assert_eq!(inherited(&model), None, "a folder without any");
        let mut canvas = Canvas::new(1080, 400);
        draw(&mut canvas, &model, 124.0, None);
    }
}
