//! The WhatsApp glyphs the shared icon set lacks (ticks, the paper plane,
//! the back arrow, ⋮ …), drawn on a 24-unit grid like it and rasterized once
//! per glyph and size. Everything else comes from `exocortex::ui::icons`.

#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::exocortex::ui::icons::{self, Icon};
use crate::gfx::{Canvas, Path, Rgb};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Glyph {
    Back,
    More,
    Tick,
    DoubleTick,
    Send,
    Smiley,
    Lock,
    PlayFill,
    PauseFill,
    Archive,
    Forward,
    Blocked,
    Chats,
    Updates,
    PhoneMissed,
    VideoMissed,
    Paperclip,
    Person,
    Group,
    Doc,
    Settings,
}

struct Mask {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    coverage: Vec<u8>,
}

type Cache = HashMap<(Glyph, u16), Rc<Option<Mask>>>;

thread_local! {
    static CACHE: RefCell<Cache> = RefCell::new(HashMap::new());
}

/// The glyph's outline on a 24-unit grid scaled to `size` px, its box at 0,0.
fn build(glyph: Glyph, size: f32) -> Path {
    let k = size / 24.0;
    let p = |x: f32, y: f32| (x * k, y * k);
    let mut path = Path::new();
    let stroke = |path: &mut Path, points: &[(f32, f32)], width: f32| {
        let points: Vec<(f32, f32)> = points.iter().map(|&(x, y)| p(x, y)).collect();
        path.stroke_polyline(&points, width * k);
    };
    let poly = |path: &mut Path, points: &[(f32, f32)]| {
        let points: Vec<(f32, f32)> = points.iter().map(|&(x, y)| p(x, y)).collect();
        path.polygon(&points);
    };
    match glyph {
        Glyph::Back => {
            stroke(&mut path, &[(20.0, 12.0), (4.5, 12.0)], 2.2);
            stroke(&mut path, &[(11.0, 5.0), (4.0, 12.0), (11.0, 19.0)], 2.2);
        }
        Glyph::More => {
            for y in [5.0, 12.0, 19.0] {
                path.circle(12.0 * k, y * k, 2.0 * k);
            }
        }
        Glyph::Tick => stroke(&mut path, &[(4.0, 12.5), (9.0, 17.0), (19.0, 7.0)], 1.9),
        Glyph::DoubleTick => {
            stroke(&mut path, &[(1.5, 12.5), (6.5, 17.0), (16.5, 7.0)], 1.9);
            stroke(&mut path, &[(11.5, 16.0), (12.5, 17.0), (22.5, 7.0)], 1.9);
        }
        Glyph::Send => poly(
            &mut path,
            &[
                (3.0, 3.5),
                (21.5, 12.0),
                (3.0, 20.5),
                (5.0, 13.2),
                (14.0, 12.0),
                (5.0, 10.8),
            ],
        ),
        Glyph::Smiley => {
            let (cx, cy, r) = (12.0 * k, 12.0 * k, 9.0 * k);
            path.circle(cx, cy, r);
            // The ring: the inner disc wound the other way cuts it out.
            let inner: Vec<(f32, f32)> = (0..48)
                .rev()
                .map(|i| {
                    let a = std::f32::consts::TAU * i as f32 / 48.0;
                    (cx + (r - 1.8 * k) * a.cos(), cy + (r - 1.8 * k) * a.sin())
                })
                .collect();
            path.polygon(&inner);
            path.circle(8.8 * k, 9.8 * k, 1.4 * k);
            path.circle(15.2 * k, 9.8 * k, 1.4 * k);
            let smile: Vec<(f32, f32)> = (0..=12)
                .map(|i| {
                    let a = 0.25 + (std::f32::consts::PI - 0.5) * i as f32 / 12.0;
                    (12.0 + 4.8 * a.cos(), 12.3 + 4.8 * a.sin())
                })
                .collect();
            stroke(&mut path, &smile, 1.8);
        }
        Glyph::Lock => {
            poly(
                &mut path,
                &[(5.0, 10.5), (19.0, 10.5), (19.0, 20.5), (5.0, 20.5)],
            );
            let arc: Vec<(f32, f32)> = (0..=16)
                .map(|i| {
                    let a = std::f32::consts::PI + std::f32::consts::PI * i as f32 / 16.0;
                    (12.0 + 4.5 * a.cos(), 9.5 + 4.5 * a.sin())
                })
                .collect();
            let mut pts = vec![(7.5, 11.0)];
            pts.extend(arc);
            pts.push((16.5, 11.0));
            stroke(&mut path, &pts, 2.0);
        }
        Glyph::PlayFill => poly(&mut path, &[(7.0, 4.5), (20.0, 12.0), (7.0, 19.5)]),
        Glyph::PauseFill => {
            poly(
                &mut path,
                &[(6.5, 5.0), (10.0, 5.0), (10.0, 19.0), (6.5, 19.0)],
            );
            poly(
                &mut path,
                &[(14.0, 5.0), (17.5, 5.0), (17.5, 19.0), (14.0, 19.0)],
            );
        }
        Glyph::Archive => {
            stroke(
                &mut path,
                &[(3.5, 5.0), (20.5, 5.0), (20.5, 9.0), (3.5, 9.0), (3.5, 5.0)],
                1.8,
            );
            stroke(
                &mut path,
                &[(5.0, 9.0), (5.0, 19.5), (19.0, 19.5), (19.0, 9.0)],
                1.8,
            );
            stroke(&mut path, &[(9.5, 12.5), (14.5, 12.5)], 1.8);
        }
        Glyph::Forward => {
            let curve: Vec<(f32, f32)> = (0..=12)
                .map(|i| {
                    let t = i as f32 / 12.0;
                    (4.0 + 15.0 * t, 19.0 - 9.0 * (1.0 - (1.0 - t) * (1.0 - t)))
                })
                .collect();
            stroke(&mut path, &curve, 2.0);
            stroke(&mut path, &[(14.5, 5.5), (19.5, 10.0), (14.5, 14.5)], 2.0);
        }
        Glyph::Blocked => {
            let ring: Vec<(f32, f32)> = (0..=40)
                .map(|i| {
                    let a = std::f32::consts::TAU * i as f32 / 40.0;
                    (12.0 + 8.0 * a.cos(), 12.0 + 8.0 * a.sin())
                })
                .collect();
            stroke(&mut path, &ring, 1.8);
            stroke(&mut path, &[(6.4, 17.6), (17.6, 6.4)], 1.8);
        }
        Glyph::Chats => {
            let ring: Vec<(f32, f32)> = (0..=36)
                .map(|i| {
                    let a = 2.3 + (std::f32::consts::TAU - 0.6) * i as f32 / 36.0;
                    (12.0 + 8.5 * a.cos(), 11.5 + 8.0 * a.sin())
                })
                .collect();
            stroke(&mut path, &ring, 2.0);
            stroke(&mut path, &[(6.2, 17.0), (4.0, 21.0), (8.5, 19.3)], 2.0);
        }
        Glyph::Updates => {
            let ring: Vec<(f32, f32)> = (0..=40)
                .map(|i| {
                    let a = std::f32::consts::TAU * i as f32 / 40.0;
                    (12.0 + 8.5 * a.cos(), 12.0 + 8.5 * a.sin())
                })
                .collect();
            stroke(&mut path, &ring, 2.0);
            path.circle(12.0 * k, 12.0 * k, 4.0 * k);
        }
        Glyph::PhoneMissed => {
            stroke(&mut path, &[(4.0, 6.0), (10.0, 12.0), (15.0, 7.0)], 2.0);
            stroke(&mut path, &[(15.0, 7.0), (15.0, 10.5)], 2.0);
            stroke(&mut path, &[(15.0, 7.0), (11.5, 7.0)], 2.0);
            let arc: Vec<(f32, f32)> = (0..=16)
                .map(|i| {
                    let a = std::f32::consts::PI + std::f32::consts::PI * i as f32 / 16.0;
                    (12.0 + 9.0 * a.cos(), 20.0 + 4.5 * a.sin())
                })
                .collect();
            stroke(&mut path, &arc, 2.2);
        }
        Glyph::VideoMissed => {
            stroke(
                &mut path,
                &[
                    (3.0, 7.0),
                    (15.0, 7.0),
                    (15.0, 17.0),
                    (3.0, 17.0),
                    (3.0, 7.0),
                ],
                1.9,
            );
            poly(
                &mut path,
                &[(16.0, 10.5), (21.0, 7.5), (21.0, 16.5), (16.0, 13.5)],
            );
        }
        Glyph::Paperclip => {
            stroke(
                &mut path,
                &[
                    (16.5, 7.0),
                    (8.0, 15.5),
                    (8.5, 18.0),
                    (11.0, 18.5),
                    (19.0, 10.5),
                    (19.0, 6.0),
                    (15.5, 4.5),
                    (6.0, 14.0),
                    (6.0, 19.0),
                    (10.5, 21.0),
                    (17.0, 14.5),
                ],
                1.7,
            );
        }
        Glyph::Person => {
            path.circle(12.0 * k, 8.5 * k, 4.6 * k);
            let shoulders: Vec<(f32, f32)> = (0..=24)
                .map(|i| {
                    let a = std::f32::consts::PI + std::f32::consts::PI * i as f32 / 24.0;
                    (12.0 + 8.5 * a.cos(), 22.0 + 8.0 * a.sin())
                })
                .collect();
            poly(&mut path, &shoulders);
        }
        Glyph::Group => {
            path.circle(9.0 * k, 9.0 * k, 3.8 * k);
            path.circle(16.5 * k, 9.5 * k, 3.2 * k);
            let a: Vec<(f32, f32)> = (0..=20)
                .map(|i| {
                    let t = std::f32::consts::PI + std::f32::consts::PI * i as f32 / 20.0;
                    (9.0 + 7.0 * t.cos(), 21.0 + 7.0 * t.sin())
                })
                .collect();
            poly(&mut path, &a);
            let b: Vec<(f32, f32)> = (0..=20)
                .map(|i| {
                    let t = std::f32::consts::PI + std::f32::consts::PI * i as f32 / 20.0;
                    (17.0 + 5.5 * t.cos(), 20.5 + 6.0 * t.sin())
                })
                .collect();
            poly(&mut path, &b);
        }
        Glyph::Doc => {
            poly(
                &mut path,
                &[
                    (5.0, 2.5),
                    (14.5, 2.5),
                    (19.0, 7.0),
                    (19.0, 21.5),
                    (5.0, 21.5),
                ],
            );
        }
        Glyph::Settings => {
            let mut points = Vec::new();
            for i in 0..48 {
                let a = std::f32::consts::TAU * i as f32 / 48.0;
                let tooth = if (i / 3) % 2 == 0 { 9.5 } else { 7.6 };
                points.push((12.0 + tooth * a.cos(), 12.0 + tooth * a.sin()));
            }
            poly(&mut path, &points);
            let inner: Vec<(f32, f32)> = (0..32)
                .rev()
                .map(|i| {
                    let a = std::f32::consts::TAU * i as f32 / 32.0;
                    (12.0 + 3.2 * a.cos(), 12.0 + 3.2 * a.sin())
                })
                .collect();
            poly(&mut path, &inner);
        }
    }
    path
}

fn mask(glyph: Glyph, size: f32) -> Rc<Option<Mask>> {
    let key = (glyph, (size * 2.0).round() as u16);
    CACHE.with(|cache| {
        cache
            .borrow_mut()
            .entry(key)
            .or_insert_with(|| {
                Rc::new(
                    build(glyph, size)
                        .rasterize()
                        .map(|(x, y, w, h, coverage)| Mask {
                            x,
                            y,
                            w,
                            h,
                            coverage,
                        }),
                )
            })
            .clone()
    })
}

/// Draws `glyph` in a `size` box centred on (`cx`, `cy`).
pub fn draw(
    canvas: &mut Canvas,
    glyph: Glyph,
    cx: f32,
    cy: f32,
    size: f32,
    color: Rgb,
    alpha: f32,
) {
    let left = (cx - size / 2.0).round() as i32;
    let top = (cy - size / 2.0).round() as i32;
    if let Some(m) = mask(glyph, size).as_ref() {
        canvas.draw_mask(left + m.x, top + m.y, m.w, m.h, &m.coverage, color, alpha);
    }
}

/// One of the shared set (lucide-style strokes).
pub fn shared(canvas: &mut Canvas, icon: Icon, cx: f32, cy: f32, size: f32, color: Rgb) {
    icons::draw(canvas, icon, cx, cy, size, color, 1.0);
}
