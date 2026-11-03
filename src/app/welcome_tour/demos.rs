//! Looping animations that illustrate each Getting Started tip. They are
//! drawn with the painter on a 400×300 logical stage scaled to fit, so they
//! stay sharp at any display scale and use the app's own colours. The one
//! image is Mira's Spout sprite, standing in for a VTuber feed. Every demo
//! is a pure function of the time into its loop.

use crate::hotkeys::{HotkeyBindings, HotkeySlot};
use crate::ui_theme;
use eframe::egui::{
    pos2, vec2, Align2, Color32, FontId, Mesh, Painter, Pos2, Rect, Rounding, Shape, Stroke,
    TextureId, Vec2,
};
use egui_phosphor::regular as icon;
use std::f32::consts::TAU;

/// Seconds one loop of most demos takes; see [`Demo::length`].
pub(super) const LOOP: f32 = 4.0;
const W: f32 = 400.0;
const H: f32 = 300.0;
/// Seconds the content fades out and back in where the loop restarts.
const SEAM: f32 = 0.25;

const CANVAS: Color32 = Color32::from_rgb(14, 15, 18);
const DOT: Color32 = Color32::from_rgb(42, 43, 50);
const SELECT: Color32 = Color32::from_rgb(74, 158, 255);
const INTERACT: Color32 = Color32::from_rgb(207, 161, 57);
const CROP: Color32 = Color32::from_rgb(255, 150, 100);
const REC: Color32 = Color32::from_rgb(235, 72, 72);
const TEAL: Color32 = Color32::from_rgb(64, 170, 160);
const VIOLET: Color32 = Color32::from_rgb(150, 110, 220);
const BLUE: Color32 = Color32::from_rgb(80, 140, 230);
const GREEN: Color32 = Color32::from_rgb(90, 190, 110);
const PALETTE: [Color32; 5] = [TEAL, VIOLET, BLUE, GREEN, Color32::from_rgb(230, 150, 80)];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Demo {
    Tiles,
    Live,
    Saved,
    AddWindow,
    AddBrowser,
    DropFiles,
    Streams,
    PanZoom,
    MoveResize,
    Select,
    Crop,
    Remove,
    UseApp,
    UsePage,
    Hover,
    TileMenu,
    Focus,
    Group,
    TileCenter,
    Pin,
    Pip,
    Screenshot,
    Record,
    ScreenshotPanel,
    StreamAudio,
    Phone,
    Viewpoints,
    Wallpaper,
    Overlay,
    HideUi,
    Reopen,
    Shortcuts,
}

impl Demo {
    #[cfg(test)]
    pub(super) const ALL: [Self; 32] = [
        Self::Tiles,
        Self::Live,
        Self::Saved,
        Self::AddWindow,
        Self::AddBrowser,
        Self::DropFiles,
        Self::Streams,
        Self::PanZoom,
        Self::MoveResize,
        Self::Select,
        Self::Crop,
        Self::Remove,
        Self::UseApp,
        Self::UsePage,
        Self::Hover,
        Self::TileMenu,
        Self::Focus,
        Self::Group,
        Self::TileCenter,
        Self::Pin,
        Self::Pip,
        Self::Screenshot,
        Self::Record,
        Self::ScreenshotPanel,
        Self::StreamAudio,
        Self::Phone,
        Self::Viewpoints,
        Self::Wallpaper,
        Self::Overlay,
        Self::HideUi,
        Self::Reopen,
        Self::Shortcuts,
    ];

    /// Seconds one loop of this demo takes. Overlay mode tells a longer story.
    pub(super) fn length(self) -> f32 {
        match self {
            Self::Overlay => LOOP * 2.0,
            _ => LOOP,
        }
    }

    /// Demos that move or replace the canvas background themselves.
    fn own_backdrop(self) -> bool {
        matches!(
            self,
            Self::PanZoom
                | Self::Pin
                | Self::Pip
                | Self::Viewpoints
                | Self::Wallpaper
                | Self::Overlay
        )
    }
}

/// Paint `demo` `time` seconds into its loop, fitted inside `area`. `mira`
/// is the sprite sheet from `assets/mira/tour-spout.webp`.
pub(super) fn paint(
    painter: &Painter,
    area: Rect,
    demo: Demo,
    time: f32,
    bindings: &HotkeyBindings,
    mira: Option<TextureId>,
) {
    let scale = (area.width() / W).min(area.height() / H).max(0.01);
    let frame = Rect::from_center_size(area.center(), vec2(W, H) * scale);
    let painter = painter.with_clip_rect(frame);
    painter.rect_filled(frame, 0.0, CANVAS);
    let mut stage = Stage {
        painter,
        origin: frame.min,
        scale,
        bindings,
        mira,
    };
    if !demo.own_backdrop() {
        stage.grid(Vec2::ZERO, 1.0);
    }

    let length = demo.length();
    let t = time.rem_euclid(length);
    let opacity = stage.painter.opacity();
    stage
        .painter
        .multiply_opacity(ramp(t, 0.0, SEAM).min(1.0 - ramp(t, length - SEAM, length)));
    let s = &mut stage;
    match demo {
        Demo::Tiles => tiles(s, t),
        Demo::Live => live(s, t),
        Demo::Saved => saved(s, t),
        Demo::AddWindow => add_window(s, t),
        Demo::AddBrowser => add_browser(s, t),
        Demo::DropFiles => drop_files(s, t),
        Demo::Streams => streams(s, t),
        Demo::PanZoom => pan_zoom(s, t),
        Demo::MoveResize => move_resize(s, t),
        Demo::Select => select(s, t),
        Demo::Crop => crop(s, t),
        Demo::Remove => remove(s, t),
        Demo::UseApp => use_app(s, t),
        Demo::UsePage => use_page(s, t),
        Demo::Hover => hover(s, t),
        Demo::TileMenu => tile_menu(s, t),
        Demo::Focus => focus(s, t),
        Demo::Group => group(s, t),
        Demo::TileCenter => tile_center(s, t),
        Demo::Pin => pin(s, t),
        Demo::Pip => pip(s, t),
        Demo::Screenshot => screenshot(s, t),
        Demo::Record => record(s, t),
        Demo::ScreenshotPanel => screenshot_panel(s, t),
        Demo::StreamAudio => stream_audio(s, t),
        Demo::Phone => phone(s, t),
        Demo::Viewpoints => viewpoints(s, t),
        Demo::Wallpaper => wallpaper(s, t),
        Demo::Overlay => overlay(s, t),
        Demo::HideUi => hide_ui(s, t),
        Demo::Reopen => reopen(s, t),
        Demo::Shortcuts => shortcuts(s, t),
    }
    stage.painter.set_opacity(opacity);

    // Round the stage's corners by covering them with the panel colour,
    // then outline it.
    let radius = 8.0;
    stage.painter.rect_stroke(
        frame.expand(radius * 0.75),
        radius * 1.75,
        Stroke::new(radius * 1.5, ui_theme::PANEL),
    );
    stage
        .painter
        .rect_stroke(frame, radius, Stroke::new(1.0, ui_theme::BORDER));
}

// ---------------------------------------------------------------------------
// Timing helpers

/// 0 before `start`, 1 after `end`, linear between.
fn ramp(t: f32, start: f32, end: f32) -> f32 {
    ((t - start) / (end - start)).clamp(0.0, 1.0)
}

/// Like [`ramp`], eased in and out.
fn ease(t: f32, start: f32, end: f32) -> f32 {
    let x = ramp(t, start, end);
    x * x * (3.0 - 2.0 * x)
}

/// Fades in at `from` and out at `to`.
fn show(t: f32, from: f32, to: f32) -> f32 {
    ramp(t, from, from + 0.15).min(1.0 - ramp(t, to - 0.15, to))
}

/// Progress of a click ripple started at `at`.
fn click(t: f32, at: f32) -> Option<f32> {
    let k = (t - at) / 0.45;
    (0.0..1.0).contains(&k).then_some(k)
}

/// Eased position along `(time, point)` keyframes.
fn track(t: f32, keys: &[(f32, Pos2)]) -> Pos2 {
    let mut at = keys[0].1;
    for pair in keys.windows(2) {
        let ((start, from), (end, to)) = (pair[0], pair[1]);
        if t >= start {
            at = from + (to - from) * ease(t, start, end);
        }
    }
    at
}

fn track_rect(t: f32, keys: &[(f32, Rect)]) -> Rect {
    let mut at = keys[0].1;
    for pair in keys.windows(2) {
        let ((start, from), (end, to)) = (pair[0], pair[1]);
        if t >= start {
            at = lerp_rect(from, to, ease(t, start, end));
        }
    }
    at
}

fn lerp_rect(a: Rect, b: Rect, k: f32) -> Rect {
    Rect::from_min_max(a.min + (b.min - a.min) * k, a.max + (b.max - a.max) * k)
}

fn scale_about(rect: Rect, k: f32) -> Rect {
    Rect::from_center_size(rect.center(), rect.size() * k)
}

fn r(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect::from_min_size(pos2(x, y), vec2(w, h))
}

/// A canvas-space rect seen through a camera panned by `offset` and zoomed
/// about the stage centre.
fn view(rect: Rect, offset: Vec2, zoom: f32) -> Rect {
    let c = pos2(W / 2.0, H / 2.0);
    Rect::from_min_max(
        c + (rect.min - c + offset) * zoom,
        c + (rect.max - c + offset) * zoom,
    )
}

// ---------------------------------------------------------------------------
// Drawing

/// What a tile shows.
#[derive(Clone, Copy)]
enum Art {
    Window(Color32),
    Browser,
    Video,
    Image,
    Chat,
    Game,
    /// Mira on a transparent background, like a VTuber Spout2 feed. Its
    /// phase counts her 4 s idle loops.
    Avatar,
}

struct Stage<'a> {
    painter: Painter,
    origin: Pos2,
    scale: f32,
    bindings: &'a HotkeyBindings,
    mira: Option<TextureId>,
}

impl Stage<'_> {
    fn pt(&self, at: Pos2) -> Pos2 {
        self.origin + at.to_vec2() * self.scale
    }

    fn rect(&self, rect: Rect) -> Rect {
        Rect::from_min_max(self.pt(rect.min), self.pt(rect.max))
    }

    fn s(&self, value: f32) -> f32 {
        value * self.scale
    }

    fn slot(&self, slot: HotkeySlot) -> String {
        self.bindings.get(slot).display()
    }

    /// Draw with extra transparency.
    fn faded(&mut self, alpha: f32, draw: impl FnOnce(&mut Self)) {
        if alpha <= 0.0 {
            return;
        }
        let before = self.painter.opacity();
        self.painter.multiply_opacity(alpha);
        draw(self);
        self.painter.set_opacity(before);
    }

    fn fill(&self, rect: Rect, rounding: f32, color: Color32) {
        self.painter
            .rect_filled(self.rect(rect), self.s(rounding), color);
    }

    fn outline(&self, rect: Rect, color: Color32) {
        self.painter.rect_stroke(
            self.rect(rect.expand(1.0)),
            self.s(5.0),
            Stroke::new(self.s(1.6), color),
        );
    }

    fn handles(&self, rect: Rect) {
        for corner in [
            rect.left_top(),
            rect.right_top(),
            rect.left_bottom(),
            rect.right_bottom(),
        ] {
            let handle = Rect::from_center_size(self.pt(corner), Vec2::splat(self.s(6.0)));
            self.painter
                .rect_filled(handle, self.s(1.5), Color32::WHITE);
            self.painter
                .rect_stroke(handle, self.s(1.5), Stroke::new(self.s(1.0), SELECT));
        }
    }

    fn text(&self, at: Pos2, align: Align2, text: &str, size: f32, color: Color32) {
        self.painter.text(
            self.pt(at),
            align,
            text,
            FontId::proportional(self.s(size)),
            color,
        );
    }

    fn grid(&self, offset: Vec2, zoom: f32) {
        let step = 20.0 * zoom;
        let c = pos2(W / 2.0, H / 2.0);
        let origin = c + (Pos2::ZERO - c + offset) * zoom;
        let first = vec2(
            ((0.0 - origin.x) / step).ceil(),
            ((0.0 - origin.y) / step).ceil(),
        );
        let radius = self.s(0.9 * zoom.min(1.5));
        let mut y = origin.y + first.y * step;
        while y <= H {
            let mut x = origin.x + first.x * step;
            while x <= W {
                self.painter.circle_filled(self.pt(pos2(x, y)), radius, DOT);
                x += step;
            }
            y += step;
        }
    }

    fn tile(&self, rect: Rect, art: Art, phase: f32) {
        self.art(rect, rect, art, phase);
    }

    /// Paint `art` laid out for `full` but only where `visible` (a crop).
    fn art(&self, full: Rect, visible: Rect, art: Art, phase: f32) {
        let painter = self.painter.with_clip_rect(self.rect(visible));
        let at = |x: f32, y: f32| {
            self.pt(pos2(
                full.min.x + x * full.width(),
                full.min.y + y * full.height(),
            ))
        };
        let area = |x0: f32, y0: f32, x1: f32, y1: f32| Rect::from_min_max(at(x0, y0), at(x1, y1));
        let unit = self.s(full.height().min(full.width()));
        let round = self.s(5.0);
        let top = Rounding {
            nw: round,
            ne: round,
            sw: 0.0,
            se: 0.0,
        };
        let bottom = Rounding {
            nw: 0.0,
            ne: 0.0,
            sw: round,
            se: round,
        };
        let bar = |rect: Rect, color: Color32| painter.rect_filled(rect, unit * 0.02, color);
        let triangle = |points: [(f32, f32); 3], color: Color32| {
            painter.add(Shape::convex_polygon(
                points.iter().map(|&(x, y)| at(x, y)).collect(),
                color,
                Stroke::NONE,
            ));
        };
        let visible_rect = self.rect(visible);

        match art {
            Art::Window(accent) => {
                painter.rect_filled(visible_rect, round, Color32::from_rgb(30, 32, 38));
                painter.rect_filled(
                    area(0.0, 0.13, 0.24, 1.0),
                    Rounding {
                        sw: round,
                        ..Rounding::ZERO
                    },
                    accent.gamma_multiply(0.22),
                );
                for i in 0..4 {
                    let y = 0.2 + i as f32 * 0.11;
                    let alpha = if i == 0 { 0.9 } else { 0.4 };
                    bar(area(0.04, y, 0.2, y + 0.05), accent.gamma_multiply(alpha));
                }
                bar(area(0.3, 0.2, 0.62, 0.27), Color32::from_rgb(200, 202, 210));
                for (i, width) in [0.6, 0.48, 0.62, 0.4, 0.55].iter().enumerate() {
                    let y = 0.36 + i as f32 * 0.11;
                    bar(
                        area(0.3, y, 0.3 + width, y + 0.045),
                        Color32::from_rgb(70, 74, 84),
                    );
                }
                painter.rect_filled(
                    area(0.0, 0.0, 1.0, 0.13),
                    top,
                    Color32::from_rgb(44, 46, 54),
                );
                for (i, color) in [(236, 106, 94), (244, 191, 79), (98, 197, 85)]
                    .into_iter()
                    .enumerate()
                {
                    painter.circle_filled(
                        at(0.04 + i as f32 * 0.035, 0.065),
                        unit * 0.022,
                        Color32::from_rgb(color.0, color.1, color.2).gamma_multiply(0.8),
                    );
                }
            }
            Art::Browser => {
                painter.rect_filled(visible_rect, round, Color32::from_rgb(232, 233, 236));
                let dy = -phase * 0.55;
                bar(
                    area(0.05, 0.2 + dy, 0.95, 0.5 + dy),
                    Color32::from_rgb(90, 130, 200),
                );
                painter.circle_filled(
                    at(0.75, 0.33 + dy),
                    unit * 0.06,
                    Color32::from_rgb(250, 220, 140),
                );
                bar(
                    area(0.05, 0.56 + dy, 0.6, 0.61 + dy),
                    Color32::from_rgb(60, 62, 70),
                );
                for (i, width) in [0.85, 0.75, 0.8, 0.7, 0.82].iter().enumerate() {
                    let y = 0.66 + i as f32 * 0.06 + if i > 2 { 0.3 } else { 0.0 };
                    bar(
                        area(0.05, y + dy, 0.05 + width, y + 0.03 + dy),
                        Color32::from_rgb(170, 172, 180),
                    );
                }
                for i in 0..3 {
                    let x = 0.05 + i as f32 * 0.31;
                    bar(
                        area(x, 0.86 + dy, x + 0.28, 1.06 + dy),
                        Color32::from_rgb(205, 208, 215),
                    );
                }
                bar(
                    area(0.05, 1.25 + dy, 0.95, 1.5 + dy),
                    Color32::from_rgb(220, 150, 90),
                );
                painter.rect_filled(
                    area(0.0, 0.0, 1.0, 0.14),
                    top,
                    Color32::from_rgb(40, 42, 50),
                );
                painter.rect_filled(
                    area(0.2, 0.035, 0.85, 0.105),
                    unit * 0.035,
                    Color32::from_rgb(64, 67, 77),
                );
                for i in 0..2 {
                    painter.circle_filled(
                        at(0.05 + i as f32 * 0.055, 0.07),
                        unit * 0.022,
                        Color32::from_rgb(110, 112, 120),
                    );
                }
            }
            Art::Video => {
                painter.rect_filled(visible_rect, round, Color32::from_rgb(26, 20, 44));
                painter.circle_filled(at(0.68, 0.4), unit * 0.16, Color32::from_rgb(240, 170, 90));
                triangle(
                    [(-0.1, 0.9), (0.35, 0.45), (0.8, 0.9)],
                    Color32::from_rgb(70, 44, 100),
                );
                triangle(
                    [(0.4, 0.9), (0.8, 0.55), (1.2, 0.9)],
                    Color32::from_rgb(100, 56, 120),
                );
                painter.rect_filled(
                    area(0.0, 0.86, 1.0, 1.0),
                    bottom,
                    Color32::from_rgb(40, 28, 62),
                );
                bar(area(0.05, 0.92, 0.95, 0.94), Color32::from_rgb(80, 80, 95));
                bar(
                    area(0.05, 0.92, 0.05 + 0.9 * phase.clamp(0.0, 1.0), 0.94),
                    ui_theme::GOLD,
                );
            }
            Art::Image => {
                painter.rect_filled(visible_rect, round, Color32::from_rgb(96, 150, 205));
                painter.circle_filled(at(0.76, 0.28), unit * 0.1, Color32::from_rgb(255, 214, 110));
                triangle(
                    [(-0.05, 0.86), (0.3, 0.35), (0.65, 0.86)],
                    Color32::from_rgb(60, 96, 92),
                );
                triangle(
                    [(0.3, 0.35), (0.255, 0.42), (0.345, 0.42)],
                    Color32::from_rgb(235, 240, 245),
                );
                triangle(
                    [(0.35, 0.86), (0.7, 0.45), (1.05, 0.86)],
                    Color32::from_rgb(84, 126, 110),
                );
                painter.rect_filled(
                    area(0.0, 0.82, 1.0, 1.0),
                    bottom,
                    Color32::from_rgb(70, 120, 80),
                );
            }
            Art::Chat => {
                painter.rect_filled(visible_rect, round, Color32::from_rgb(27, 28, 33));
                let scroll = phase.rem_euclid(1.0) * 0.11;
                for i in 0..10 {
                    let y = 0.17 + i as f32 * 0.11 - scroll;
                    let color = PALETTE[(i + phase.floor() as usize) % PALETTE.len()];
                    painter.circle_filled(at(0.08, y + 0.035), unit * 0.03, color);
                    bar(area(0.15, y, 0.32, y + 0.035), color.gamma_multiply(0.85));
                    let width = [0.7, 0.5, 0.62, 0.42][i % 4];
                    bar(
                        area(0.15, y + 0.05, 0.15 + width, y + 0.08),
                        Color32::from_rgb(80, 82, 92),
                    );
                }
                painter.rect_filled(
                    area(0.0, 0.0, 1.0, 0.12),
                    top,
                    Color32::from_rgb(38, 39, 46),
                );
                bar(
                    area(0.06, 0.045, 0.4, 0.075),
                    Color32::from_rgb(150, 152, 160),
                );
            }
            Art::Game => {
                painter.rect_filled(visible_rect, round, Color32::from_rgb(60, 92, 145));
                for (x, y) in [(0.2, 0.2), (0.62, 0.14), (0.86, 0.3)] {
                    let cloud = Color32::from_rgba_unmultiplied(255, 255, 255, 60);
                    painter.circle_filled(at(x, y), unit * 0.05, cloud);
                    painter.circle_filled(at(x + 0.05, y + 0.01), unit * 0.04, cloud);
                }
                triangle(
                    [(0.0, 0.76), (0.2, 0.5), (0.42, 0.76)],
                    Color32::from_rgb(48, 100, 70),
                );
                triangle(
                    [(0.55, 0.76), (0.78, 0.46), (1.0, 0.76)],
                    Color32::from_rgb(48, 100, 70),
                );
                painter.rect_filled(
                    area(0.0, 0.75, 1.0, 1.0),
                    bottom,
                    Color32::from_rgb(76, 150, 84),
                );
                bar(area(0.55, 0.55, 0.8, 0.6), Color32::from_rgb(150, 105, 60));
                for x in [0.6, 0.68, 0.76] {
                    painter.circle_filled(
                        at(x, 0.48),
                        unit * 0.02,
                        Color32::from_rgb(250, 210, 80),
                    );
                }
                let run = phase.rem_euclid(1.0);
                let jump = (run * TAU * 2.0).sin().abs() * 0.12;
                let x = 0.1 + 0.75 * run;
                bar(
                    area(x, 0.64 - jump, x + 0.06, 0.75 - jump),
                    Color32::from_rgb(240, 90, 80),
                );
            }
            Art::Avatar => {
                if let Some(texture) = self.mira {
                    mira(&painter, texture, self.rect(full), phase);
                }
            }
        }
    }

    /// The pointer, with a ripple while `click` is running.
    fn cursor(&self, at: Pos2, click: Option<f32>) {
        let origin = self.pt(at);
        if let Some(k) = click {
            self.painter.circle_stroke(
                origin,
                self.s(4.0 + 12.0 * k),
                Stroke::new(self.s(1.6), ui_theme::GOLD.gamma_multiply(1.0 - k)),
            );
        }
        let point = |x: f32, y: f32| origin + vec2(x, y) * self.scale;
        let fill = Color32::WHITE;
        for points in [
            vec![point(0.0, 0.0), point(0.0, 15.0), point(3.6, 11.6)],
            vec![point(0.0, 0.0), point(3.6, 11.6), point(11.0, 10.8)],
            vec![
                point(3.6, 11.6),
                point(6.4, 17.6),
                point(8.8, 16.6),
                point(6.0, 10.8),
            ],
        ] {
            self.painter
                .add(Shape::convex_polygon(points, fill, Stroke::NONE));
        }
        self.painter.add(Shape::closed_line(
            vec![
                point(0.0, 0.0),
                point(0.0, 15.0),
                point(3.6, 11.6),
                point(6.4, 17.6),
                point(8.8, 16.6),
                point(6.0, 10.8),
                point(11.0, 10.8),
            ],
            Stroke::new(self.s(1.0), Color32::from_rgb(20, 20, 24)),
        ));
    }

    /// A small icon beside the pointer, such as the mouse button in use.
    fn cursor_hint(&self, at: Pos2, glyph: &str, alpha: f32) {
        if alpha <= 0.0 {
            return;
        }
        let center = at + vec2(22.0, 22.0);
        let mut painter = self.painter.clone();
        painter.multiply_opacity(alpha);
        painter.circle_filled(self.pt(center), self.s(10.0), ui_theme::PANEL);
        painter.text(
            self.pt(center),
            Align2::CENTER_CENTER,
            glyph,
            FontId::proportional(self.s(13.0)),
            ui_theme::GOLD,
        );
    }

    /// A pressed shortcut, shown at the bottom of the stage.
    fn key(&self, label: &str, alpha: f32) {
        if alpha <= 0.0 {
            return;
        }
        let mut painter = self.painter.clone();
        painter.multiply_opacity(alpha);
        let galley = painter.layout_no_wrap(
            label.to_owned(),
            FontId::monospace(self.s(13.0)),
            ui_theme::TEXT,
        );
        let size = galley.size() + vec2(self.s(18.0), self.s(10.0));
        let rect = Rect::from_center_size(self.pt(pos2(W / 2.0, H - 24.0)), size);
        painter.rect(
            rect,
            self.s(6.0),
            ui_theme::PANEL,
            Stroke::new(self.s(1.2), ui_theme::GOLD),
        );
        painter.galley(rect.center() - galley.size() / 2.0, galley, ui_theme::TEXT);
    }

    fn menu(&self, at: Pos2, items: &[&str], highlight: Option<usize>) {
        const WIDTH: f32 = 120.0;
        const ROW: f32 = 17.0;
        let menu = r(at.x, at.y, WIDTH, ROW * items.len() as f32 + 8.0);
        self.fill(
            menu.translate(vec2(2.0, 3.0)),
            6.0,
            Color32::from_black_alpha(90),
        );
        self.painter.rect(
            self.rect(menu),
            self.s(6.0),
            ui_theme::PANEL,
            Stroke::new(self.s(1.0), ui_theme::BORDER),
        );
        for (i, item) in items.iter().enumerate() {
            let row = r(at.x + 4.0, at.y + 4.0 + i as f32 * ROW, WIDTH - 8.0, ROW);
            let lit = highlight == Some(i);
            if lit {
                self.fill(row, 4.0, ui_theme::GOLD_DARK);
            }
            self.text(
                row.left_center() + vec2(6.0, 0.0),
                Align2::LEFT_CENTER,
                item,
                9.5,
                if lit { ui_theme::GOLD } else { ui_theme::TEXT },
            );
        }
    }

    fn toast(&self, glyph: &str, text: &str) {
        let rect = r(W - 156.0, H - 50.0, 142.0, 30.0);
        self.painter.rect(
            self.rect(rect),
            self.s(8.0),
            ui_theme::SURFACE,
            Stroke::new(self.s(1.0), ui_theme::BORDER),
        );
        self.text(
            rect.left_center() + vec2(14.0, 0.0),
            Align2::CENTER_CENTER,
            glyph,
            14.0,
            ui_theme::GOLD,
        );
        self.text(
            rect.left_center() + vec2(28.0, 0.0),
            Align2::LEFT_CENTER,
            text,
            10.5,
            ui_theme::TEXT,
        );
    }

    /// A floating window with a title bar; returns its content area.
    fn window_frame(&self, rect: Rect, title: &str) -> Rect {
        self.fill(
            rect.translate(vec2(3.0, 4.0)),
            7.0,
            Color32::from_black_alpha(100),
        );
        self.painter.rect(
            self.rect(rect),
            self.s(7.0),
            ui_theme::PANEL,
            Stroke::new(self.s(1.0), ui_theme::BORDER),
        );
        self.text(
            rect.min + vec2(9.0, 10.0),
            Align2::LEFT_CENTER,
            title,
            9.0,
            ui_theme::SECONDARY,
        );
        Rect::from_min_max(rect.min + vec2(0.0, 20.0), rect.max)
    }

    fn file_card(&self, center: Pos2, glyph: &str) {
        let card = Rect::from_center_size(center, vec2(28.0, 36.0));
        self.fill(
            card.translate(vec2(2.0, 2.0)),
            3.0,
            Color32::from_black_alpha(90),
        );
        self.fill(card, 3.0, Color32::from_rgb(232, 232, 236));
        self.text(
            card.center(),
            Align2::CENTER_CENTER,
            glyph,
            15.0,
            Color32::from_rgb(70, 84, 110),
        );
    }

    fn gradient(&self, rect: Rect, top: Color32, bottom: Color32) {
        let rect = self.rect(rect);
        let mut mesh = Mesh::default();
        mesh.colored_vertex(rect.left_top(), top);
        mesh.colored_vertex(rect.right_top(), top);
        mesh.colored_vertex(rect.left_bottom(), bottom);
        mesh.colored_vertex(rect.right_bottom(), bottom);
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(1, 2, 3);
        self.painter.add(Shape::mesh(mesh));
    }

    /// A pill naming a saved viewpoint.
    fn bookmark(&self, at: Pos2, number: &str) {
        let pill = r(at.x, at.y, 34.0, 18.0);
        self.fill(pill, 9.0, ui_theme::GOLD_DARK);
        self.text(
            pill.left_center() + vec2(10.0, 0.0),
            Align2::CENTER_CENTER,
            icon::BOOKMARK_SIMPLE,
            10.0,
            ui_theme::GOLD,
        );
        self.text(
            pill.left_center() + vec2(23.0, 0.0),
            Align2::CENTER_CENTER,
            number,
            10.0,
            ui_theme::GOLD,
        );
    }
}

// ---------------------------------------------------------------------------
// Welcome

fn tiles(s: &mut Stage, t: f32) {
    let items = [
        (r(28.0, 36.0, 180.0, 118.0), Art::Window(TEAL), 0.2),
        (r(222.0, 36.0, 150.0, 118.0), Art::Browser, 0.55),
        (r(28.0, 168.0, 120.0, 96.0), Art::Video, 0.9),
        (r(160.0, 168.0, 100.0, 96.0), Art::Image, 1.25),
        (r(272.0, 168.0, 100.0, 96.0), Art::Chat, 1.6),
    ];
    for (rect, art, at) in items {
        let k = ease(t, at, at + 0.35);
        s.faded(k, |s| {
            s.tile(scale_about(rect, 0.75 + 0.25 * k), art, t / LOOP)
        });
    }
}

fn live(s: &mut Stage, t: f32) {
    s.tile(r(24.0, 36.0, 170.0, 228.0), Art::Chat, t * 1.2);
    let video = r(206.0, 36.0, 170.0, 122.0);
    s.tile(video, Art::Video, t / LOOP);
    s.tile(r(206.0, 170.0, 170.0, 94.0), Art::Game, t / LOOP * 2.0);
    let pulse = 0.5 + 0.5 * (t * TAU).sin();
    let badge = r(video.min.x + 6.0, video.min.y + 6.0, 38.0, 15.0);
    s.fill(badge, 4.0, REC.gamma_multiply(0.55 + 0.45 * pulse));
    s.text(
        badge.center(),
        Align2::CENTER_CENTER,
        "LIVE",
        8.5,
        Color32::WHITE,
    );
}

fn saved(s: &mut Stage, t: f32) {
    s.tile(r(30.0, 40.0, 160.0, 110.0), Art::Window(BLUE), 0.0);
    let from = r(40.0, 170.0, 130.0, 90.0);
    let to = r(220.0, 120.0, 140.0, 96.0);
    let tile = lerp_rect(from, to, ease(t, 0.7, 1.7));
    s.tile(tile, Art::Video, 0.3);
    if (0.6..1.8).contains(&t) {
        s.outline(tile, SELECT);
    }
    let grab = vec2(60.0, 40.0);
    let cursor = if t < 0.6 {
        track(t, &[(0.0, pos2(300.0, 60.0)), (0.55, from.min + grab)])
    } else if t < 1.8 {
        tile.min + grab
    } else {
        track(t, &[(1.8, to.min + grab), (2.4, pos2(330.0, 90.0))])
    };
    s.cursor(cursor, None);
    let alpha = show(t, 2.0, 3.8);
    s.faded(alpha, |s| s.toast(icon::CHECK_CIRCLE, "Layout saved"));
}

fn reopen(s: &mut Stage, t: f32) {
    s.tile(r(40.0, 64.0, 150.0, 100.0), Art::Window(VIOLET), 0.0);
    s.tile(r(220.0, 100.0, 140.0, 110.0), Art::Image, 0.0);
    s.fill(r(0.0, 0.0, W, 24.0), 0.0, ui_theme::PANEL);
    let open = (1.05..2.45).contains(&t);
    for (name, x) in [
        ("Workspace", 12.0),
        ("View", 72.0),
        ("Tools", 106.0),
        ("Help", 146.0),
    ] {
        if name == "Help" && open {
            s.fill(r(x - 6.0, 3.0, 36.0, 18.0), 4.0, ui_theme::HOVER);
        }
        s.text(
            pos2(x, 12.0),
            Align2::LEFT_CENTER,
            name,
            10.0,
            ui_theme::TEXT,
        );
    }
    if open {
        s.menu(
            pos2(140.0, 24.0),
            &["Getting Started", "Keyboard Shortcuts", "About"],
            (t > 1.85).then_some(0),
        );
    }
    let card = scale_about(
        r(110.0, 70.0, 180.0, 150.0),
        0.85 + 0.15 * ease(t, 2.5, 2.8),
    );
    let alpha = ramp(t, 2.5, 2.7);
    s.faded(alpha, |s| {
        let content = s.window_frame(card, "Getting Started");
        let badge = r(content.min.x + 10.0, content.min.y + 8.0, 24.0, 24.0);
        s.fill(badge, 5.0, ui_theme::GOLD_DARK);
        s.text(
            badge.center(),
            Align2::CENTER_CENTER,
            icon::HAND_WAVING,
            13.0,
            ui_theme::GOLD,
        );
        s.fill(
            r(content.min.x + 42.0, content.min.y + 12.0, 90.0, 6.0),
            2.0,
            ui_theme::TEXT,
        );
        for i in 0..4 {
            let y = content.min.y + 44.0 + i as f32 * 16.0;
            s.fill(
                r(content.min.x + 12.0, y, 10.0, 10.0),
                2.0,
                ui_theme::SURFACE,
            );
            s.fill(
                r(content.min.x + 28.0, y + 2.0, 110.0, 5.0),
                2.0,
                ui_theme::SECONDARY,
            );
        }
    });
    let cursor = track(
        t,
        &[
            (0.0, pos2(300.0, 220.0)),
            (0.95, pos2(158.0, 14.0)),
            (1.4, pos2(158.0, 14.0)),
            (1.85, pos2(190.0, 37.0)),
            (2.6, pos2(190.0, 37.0)),
            (3.2, pos2(330.0, 250.0)),
        ],
    );
    s.cursor(cursor, click(t, 1.0).or(click(t, 2.3)));
}

// ---------------------------------------------------------------------------
// Adding sources

fn add_window(s: &mut Stage, t: f32) {
    s.tile(r(200.0, 30.0, 170.0, 100.0), Art::Image, 0.0);
    let x = -120.0 + 120.0 * ease(t, 0.25, 0.7);
    s.fill(r(x, 0.0, 120.0, H), 0.0, ui_theme::PANEL);
    s.fill(r(x + 119.0, 0.0, 1.0, H), 0.0, ui_theme::BORDER);
    s.text(
        pos2(x + 10.0, 14.0),
        Align2::LEFT_CENTER,
        "Windows",
        10.5,
        ui_theme::TEXT,
    );
    let row = |i: usize| r(x + 6.0, 30.0 + i as f32 * 32.0, 108.0, 28.0);
    let plus = |i: usize| row(i).right_center() - vec2(12.0, 0.0);
    for (i, color) in PALETTE.iter().enumerate() {
        let rect = row(i);
        if i == 2 && t > 1.0 {
            s.fill(rect, 5.0, ui_theme::SURFACE);
        }
        s.fill(
            r(rect.min.x + 5.0, rect.min.y + 6.0, 16.0, 16.0),
            3.0,
            *color,
        );
        s.fill(
            r(rect.min.x + 26.0, rect.min.y + 8.0, 52.0, 5.0),
            2.0,
            ui_theme::TEXT,
        );
        s.fill(
            r(rect.min.x + 26.0, rect.min.y + 17.0, 36.0, 4.0),
            2.0,
            ui_theme::SECONDARY,
        );
        s.painter
            .circle_filled(s.pt(plus(i)), s.s(7.0), ui_theme::GOLD_DARK);
        s.text(plus(i), Align2::CENTER_CENTER, "+", 11.0, ui_theme::GOLD);
    }
    if t >= 1.6 {
        let tile = lerp_rect(row(2), r(160.0, 150.0, 210.0, 125.0), ease(t, 1.6, 2.3));
        let alpha = ramp(t, 1.6, 1.75);
        s.faded(alpha, |s| s.tile(tile, Art::Window(TEAL), t / LOOP));
    }
    let cursor = track(
        t,
        &[
            (0.0, pos2(250.0, 250.0)),
            (0.8, pos2(250.0, 250.0)),
            (1.4, plus(2)),
            (2.0, plus(2)),
            (2.6, pos2(330.0, 140.0)),
        ],
    );
    s.cursor(cursor, click(t, 1.5));
    let label = s.slot(HotkeySlot::ToggleWindowPicker);
    s.key(&label, show(t, 0.05, 0.9));
}

fn add_browser(s: &mut Stage, t: f32) {
    s.tile(r(24.0, 30.0, 140.0, 96.0), Art::Chat, 0.0);
    let at = pos2(180.0, 100.0);
    if t >= 1.9 {
        let tile = lerp_rect(
            Rect::from_min_size(at, Vec2::ZERO),
            r(176.0, 60.0, 200.0, 150.0),
            ease(t, 1.9, 2.4),
        );
        s.tile(tile, Art::Browser, ramp(t, 2.6, 3.6) * 0.4);
    }
    if (0.8..1.9).contains(&t) {
        s.menu(
            at + vec2(4.0, 4.0),
            &["Add Window...", "Add Browser...", "Control Tile Center"],
            (t > 1.45).then_some(1),
        );
    }
    let cursor = track(
        t,
        &[
            (0.0, pos2(330.0, 250.0)),
            (0.6, at),
            (1.2, at),
            (1.6, at + vec2(46.0, 29.0)),
            (2.2, at + vec2(46.0, 29.0)),
            (2.8, pos2(110.0, 250.0)),
        ],
    );
    s.cursor(cursor, click(t, 0.7).or(click(t, 1.75)));
}

fn drop_files(s: &mut Stage, t: f32) {
    s.tile(r(220.0, 24.0, 150.0, 100.0), Art::Window(BLUE), 0.0);
    let path = track(
        t,
        &[
            (0.0, pos2(-30.0, 250.0)),
            (0.3, pos2(-30.0, 250.0)),
            (1.5, pos2(170.0, 160.0)),
        ],
    );
    if t < 1.6 {
        s.file_card(path + vec2(-8.0, -6.0), icon::IMAGE);
        s.file_card(path + vec2(6.0, 4.0), icon::FILM_STRIP);
        s.cursor(path + vec2(8.0, 8.0), None);
    } else {
        let k = ease(t, 1.6, 2.1);
        let start = Rect::from_center_size(path, vec2(28.0, 36.0));
        s.tile(
            lerp_rect(start, r(30.0, 120.0, 160.0, 110.0), k),
            Art::Image,
            0.0,
        );
        s.tile(
            lerp_rect(start, r(204.0, 140.0, 170.0, 112.0), k),
            Art::Video,
            ramp(t, 2.1, LOOP),
        );
        let cursor = track(
            t,
            &[(1.6, path + vec2(8.0, 8.0)), (2.4, pos2(330.0, 280.0))],
        );
        s.cursor(cursor, click(t, 1.6));
    }
}

/// Mira's Spout idle loop (`output/imagegen/mira/spout-idle-v3`) rebuilt
/// from its layers while she sways: her open-eyed drawing, with the
/// half-closed and closed-smile faces faded over it on the sprite's own
/// schedule. `phase` counts 4 s loops. She stands at the bottom of `rect`,
/// cut off by its edge like a webcam frame.
///
/// The sheet (`assets/mira/tour-spout.webp`, built by
/// `output/imagegen/mira/tour-sprite/build.py`) holds the 384×512 drawing,
/// then the two faces as opaque crops stacked to its right. Opaque, because
/// soft alpha edges glow when egui_glow draws the sprite small.
fn mira(painter: &Painter, texture: TextureId, rect: Rect, phase: f32) {
    const SHEET: Vec2 = vec2(579.0, 512.0);
    const CELL: Vec2 = vec2(384.0, 512.0);
    /// Where the faces go in the drawing, in its pixels.
    const FACE: Rect = Rect::from_min_max(pos2(137.0, 151.0), pos2(284.0, 260.0));
    /// Each face crop's top-left in the sheet, and the gap to the next.
    const CROPS: Vec2 = vec2(416.0, 16.0);
    const STEP: f32 = 157.0;

    let seconds = phase.rem_euclid(1.0) * 4.0;
    let scale = (rect.width() / CELL.x).min(rect.height() / CELL.y);
    let angle = phase * TAU;
    let sway = vec2(1.2 * angle.sin(), -1.6 * (1.0 - angle.cos())) * scale;
    let size = CELL * scale;
    let origin = pos2(rect.center().x - size.x / 2.0, rect.max.y - size.y) + sway;
    let uv = |min: Pos2, size: Vec2| {
        Rect::from_min_size(
            pos2(min.x / SHEET.x, min.y / SHEET.y),
            vec2(size.x / SHEET.x, size.y / SHEET.y),
        )
    };
    painter.image(
        texture,
        Rect::from_min_size(origin, size),
        uv(Pos2::ZERO, CELL),
        Color32::WHITE,
    );

    let half = ramp(seconds, 1.35, 1.45).min(1.0 - ramp(seconds, 3.0, 3.15));
    let closed = ramp(seconds, 1.45, 1.55).min(1.0 - ramp(seconds, 2.85, 3.0));
    let face = Rect::from_min_size(origin + FACE.min.to_vec2() * scale, FACE.size() * scale);
    for (index, alpha) in [half, closed].into_iter().enumerate() {
        if alpha > 0.0 {
            let crop = CROPS + vec2(0.0, STEP * index as f32);
            painter.image(
                texture,
                face,
                uv(crop.to_pos2(), FACE.size()),
                Color32::WHITE.gamma_multiply(alpha),
            );
        }
    }
}

fn streams(s: &mut Stage, t: f32) {
    let page = r(14.0, 44.0, 248.0, 176.0);
    s.tile(page, Art::Browser, 0.0);
    let video = r(page.min.x + 8.0, page.min.y + 32.0, 158.0, 106.0);
    s.tile(video, Art::Video, t / LOOP);
    s.tile(
        r(page.min.x + 172.0, page.min.y + 32.0, 68.0, 136.0),
        Art::Chat,
        t * 1.4,
    );
    let pulse = 0.5 + 0.5 * (t * TAU).sin();
    let badge = r(video.min.x + 6.0, video.min.y + 6.0, 34.0, 14.0);
    s.fill(badge, 4.0, REC.gamma_multiply(0.55 + 0.45 * pulse));
    s.text(
        badge.center(),
        Align2::CENTER_CENTER,
        "LIVE",
        8.0,
        Color32::WHITE,
    );
    s.tile(r(271.0, 50.0, 120.0, 160.0), Art::Avatar, t / LOOP);
    s.text(
        pos2(331.0, 230.0),
        Align2::CENTER_CENTER,
        "Spout2",
        10.0,
        ui_theme::SECONDARY,
    );
}

// ---------------------------------------------------------------------------
// Arranging

fn pan_zoom(s: &mut Stage, t: f32) {
    let offset = vec2(-70.0, -30.0) * ease(t, 0.4, 1.7);
    let zoom = 1.0 + 0.45 * ease(t, 2.1, 3.3);
    s.grid(offset, zoom);
    for (rect, art) in [
        (r(60.0, 60.0, 140.0, 95.0), Art::Window(TEAL)),
        (r(230.0, 80.0, 120.0, 90.0), Art::Image),
        (r(110.0, 180.0, 130.0, 85.0), Art::Video),
        (r(300.0, 200.0, 130.0, 90.0), Art::Chat),
        (r(-60.0, 170.0, 120.0, 80.0), Art::Browser),
        (r(380.0, 40.0, 110.0, 90.0), Art::Game),
    ] {
        s.tile(view(rect, offset, zoom), art, 0.0);
    }
    let cursor = pos2(260.0, 170.0) + offset;
    s.cursor(cursor, None);
    s.cursor_hint(cursor, icon::MOUSE_MIDDLE_CLICK, show(t, 0.2, 1.9));
    s.cursor_hint(cursor, icon::MOUSE_SCROLL, show(t, 2.0, 3.6));
}

fn move_resize(s: &mut Stage, t: f32) {
    s.tile(r(260.0, 30.0, 120.0, 80.0), Art::Image, 0.0);
    let start = r(40.0, 110.0, 150.0, 100.0);
    let moved = start.translate(vec2(50.0, -40.0));
    let grown = Rect::from_min_size(moved.min, moved.size() * 1.45);
    let rect = track_rect(t, &[(0.6, start), (1.5, moved), (2.2, moved), (3.1, grown)]);
    s.tile(rect, Art::Window(VIOLET), 0.0);
    if t > 0.5 {
        s.outline(rect, SELECT);
        s.handles(rect);
    }
    let grab = start.center() - start.min;
    let cursor = if t < 0.6 {
        track(t, &[(0.0, pos2(330.0, 260.0)), (0.55, start.center())])
    } else if t < 1.5 {
        rect.min + grab
    } else if t < 2.2 {
        track(t, &[(1.5, moved.min + grab), (2.1, moved.max)])
    } else {
        rect.max
    };
    s.cursor(cursor, None);
}

fn select(s: &mut Stage, t: f32) {
    let tiles = [
        (r(40.0, 60.0, 110.0, 80.0), Art::Window(TEAL)),
        (r(170.0, 100.0, 110.0, 80.0), Art::Video),
        (r(296.0, 170.0, 90.0, 80.0), Art::Image),
    ];
    let anchor = pos2(25.0, 40.0);
    let end = track(t, &[(0.4, anchor), (1.5, pos2(285.0, 195.0))]);
    let dragging = (0.4..1.65).contains(&t);
    let selection = Rect::from_two_pos(anchor, end);
    for (i, (rect, art)) in tiles.into_iter().enumerate() {
        s.tile(rect, art, 0.0);
        let boxed = t >= 0.4 && selection.intersects(rect);
        let all = t >= 2.4;
        if boxed || all || (i < 2 && t >= 1.65) {
            s.outline(rect, SELECT);
        }
    }
    if dragging {
        s.fill(selection, 2.0, SELECT.gamma_multiply(0.12));
        s.painter
            .rect_stroke(s.rect(selection), s.s(2.0), Stroke::new(s.s(1.0), SELECT));
    }
    let cursor = if t < 0.4 {
        track(t, &[(0.0, pos2(200.0, 270.0)), (0.35, anchor)])
    } else {
        end
    };
    s.cursor(cursor, None);
    let label = s.slot(HotkeySlot::SelectAll);
    s.key(&label, show(t, 2.2, 3.4));
}

fn crop(s: &mut Stage, t: f32) {
    let full = r(70.0, 50.0, 260.0, 180.0);
    let right = 330.0 - 90.0 * ease(t, 0.9, 1.6);
    let top = 50.0 + 50.0 * ease(t, 2.0, 2.7);
    let visible = Rect::from_min_max(pos2(70.0, top), pos2(right, 230.0));
    s.faded(0.18, |s| s.tile(full, Art::Image, 0.0));
    s.art(full, visible, Art::Image, 0.0);
    if t > 0.3 {
        s.outline(visible, CROP);
    }
    let mid = (70.0 + right) / 2.0;
    let cursor = if t < 0.9 {
        track(t, &[(0.0, pos2(370.0, 270.0)), (0.7, pos2(330.0, 140.0))])
    } else if t < 1.6 {
        pos2(right, 140.0)
    } else if t < 2.0 {
        track(t, &[(1.6, pos2(right, 140.0)), (1.95, pos2(mid, 50.0))])
    } else {
        pos2(mid, top)
    };
    s.cursor(cursor, None);
    s.key("Alt", show(t, 0.3, 3.2));
}

fn remove(s: &mut Stage, t: f32) {
    s.tile(r(40.0, 70.0, 140.0, 100.0), Art::Chat, 0.0);
    let target = r(210.0, 80.0, 150.0, 110.0);
    let gone = ease(t, 1.0, 1.35);
    let back = ease(t, 2.3, 2.7);
    let shown = 1.0 - gone * (1.0 - back);
    s.faded(shown, |s| {
        let rect = scale_about(target, 0.8 + 0.2 * shown);
        s.tile(rect, Art::Video, t / LOOP);
        if t > 0.5 {
            s.outline(rect, SELECT);
        }
    });
    let cursor = track(
        t,
        &[
            (0.0, pos2(330.0, 260.0)),
            (0.45, target.center()),
            (1.0, target.center()),
            (1.5, pos2(300.0, 250.0)),
        ],
    );
    s.cursor(cursor, click(t, 0.5));
    let delete = s.slot(HotkeySlot::DeleteSelected);
    let undo = s.slot(HotkeySlot::UndoRemovedTiles);
    s.key(&delete, show(t, 0.8, 1.6));
    s.key(&undo, show(t, 2.1, 2.9));
}

// ---------------------------------------------------------------------------
// Using tiles

/// The rect of list row `i` in a window tile drawn over `full`.
fn window_row(full: Rect, i: usize) -> Rect {
    let y = full.min.y + (0.33 + i as f32 * 0.11) * full.height();
    Rect::from_min_size(
        pos2(full.min.x + 0.28 * full.width(), y),
        vec2(0.68 * full.width(), 0.09 * full.height()),
    )
}

fn use_app(s: &mut Stage, t: f32) {
    let tile = r(70.0, 70.0, 260.0, 176.0);
    s.tile(tile, Art::Window(GREEN), 0.0);
    if t >= 1.9 {
        s.fill(
            window_row(tile, 2),
            3.0,
            ui_theme::GOLD.gamma_multiply(0.35),
        );
    }
    if (0.95..3.15).contains(&t) {
        s.painter.rect_stroke(
            s.rect(tile.expand(1.5)),
            s.s(5.0),
            Stroke::new(s.s(2.0), INTERACT),
        );
        let bar = r(tile.min.x, tile.min.y - 22.0, tile.width(), 18.0);
        s.fill(bar, 4.0, ui_theme::PANEL);
        s.text(
            bar.left_center() + vec2(8.0, 0.0),
            Align2::LEFT_CENTER,
            "Using Music",
            9.0,
            ui_theme::SECONDARY,
        );
        let done = r(bar.max.x - 42.0, bar.min.y + 2.0, 38.0, 14.0);
        s.fill(done, 4.0, ui_theme::GOLD);
        s.text(
            done.center(),
            Align2::CENTER_CENTER,
            "Done",
            8.5,
            ui_theme::PANEL,
        );
    }
    let row = window_row(tile, 2).center();
    let cursor = track(
        t,
        &[
            (0.0, pos2(350.0, 280.0)),
            (0.5, tile.center()),
            (1.2, tile.center()),
            (1.75, row),
            (2.5, row),
            (3.0, pos2(30.0, 280.0)),
        ],
    );
    let clicks = click(t, 0.6)
        .or(click(t, 0.78))
        .or(click(t, 1.85))
        .or(click(t, 3.05));
    s.cursor(cursor, clicks);
}

fn use_page(s: &mut Stage, t: f32) {
    let tile = r(60.0, 40.0, 280.0, 200.0);
    s.tile(tile, Art::Browser, ease(t, 1.1, 2.6));
    if (0.75..3.1).contains(&t) {
        s.painter.rect_stroke(
            s.rect(tile.expand(1.5)),
            s.s(5.0),
            Stroke::new(s.s(2.0), INTERACT),
        );
    }
    let cursor = track(t, &[(0.0, pos2(350.0, 270.0)), (0.9, pos2(210.0, 170.0))]);
    s.cursor(cursor, None);
    s.cursor_hint(cursor, icon::MOUSE_SCROLL, show(t, 1.0, 2.7));
    let interact = s.slot(HotkeySlot::InteractBrowser);
    let exit = s.slot(HotkeySlot::ExitTileOrBrowser);
    s.key(&interact, show(t, 0.3, 1.0));
    s.key(&exit, show(t, 2.8, 3.6));
}

fn hover(s: &mut Stage, t: f32) {
    let tile = r(70.0, 50.0, 260.0, 170.0);
    let paused_at = 2.35;
    let progress = 0.1 + 0.8 * t.min(paused_at) / LOOP;
    s.tile(tile, Art::Video, progress);
    let bar = r(tile.min.x, tile.max.y - 30.0, tile.width(), 30.0);
    let play = bar.left_center() + vec2(16.0, 0.0);
    let alpha = ramp(t, 1.0, 1.3);
    s.faded(alpha, |s| {
        s.painter.rect_filled(
            s.rect(bar),
            Rounding {
                sw: s.s(5.0),
                se: s.s(5.0),
                ..Rounding::ZERO
            },
            Color32::from_black_alpha(170),
        );
        let glyph = if t < paused_at {
            icon::PAUSE
        } else {
            icon::PLAY
        };
        s.text(play, Align2::CENTER_CENTER, glyph, 13.0, Color32::WHITE);
        let track_rect = r(
            bar.min.x + 32.0,
            bar.center().y - 1.5,
            bar.width() - 72.0,
            3.0,
        );
        s.fill(track_rect, 1.5, Color32::from_gray(110));
        let knob = track_rect.min.x + track_rect.width() * progress;
        s.fill(
            Rect::from_min_max(track_rect.min, pos2(knob, track_rect.max.y)),
            1.5,
            ui_theme::GOLD,
        );
        s.painter.circle_filled(
            s.pt(pos2(knob, track_rect.center().y)),
            s.s(4.0),
            ui_theme::GOLD,
        );
        s.text(
            bar.right_center() - vec2(18.0, 0.0),
            Align2::CENTER_CENTER,
            icon::SPEAKER_HIGH,
            13.0,
            Color32::WHITE,
        );
    });
    let cursor = track(
        t,
        &[
            (0.2, pos2(370.0, 280.0)),
            (1.1, pos2(200.0, 130.0)),
            (1.8, pos2(200.0, 130.0)),
            (2.25, play),
        ],
    );
    s.cursor(cursor, click(t, 2.35));
}

fn tile_menu(s: &mut Stage, t: f32) {
    s.tile(r(30.0, 50.0, 210.0, 150.0), Art::Window(BLUE), 0.0);
    let at = pos2(150.0, 90.0);
    let items = [
        "Interact",
        "Focus on This Tile",
        "Picture in Picture",
        "Pin to Viewport",
        "Arrange",
        "Tile Settings",
        "Freeze Tile",
        "Remove",
    ];
    let menu = at + vec2(4.0, 4.0);
    let last = items.len() as f32 - 1.0;
    // The pointer glides down the menu, lighting each item it passes.
    let step = ((t - 1.4) / 0.4).clamp(0.0, last);
    let first_row = pos2(menu.x + 70.0, menu.y + 12.5);
    if t >= 0.8 {
        let lit = (t >= 1.3).then_some(step.round() as usize);
        s.menu(menu, &items, lit);
    }
    let cursor = if t < 1.4 {
        track(
            t,
            &[
                (0.0, pos2(330.0, 260.0)),
                (0.6, at),
                (1.2, at),
                (1.4, first_row),
            ],
        )
    } else {
        first_row + vec2(0.0, 17.0 * step)
    };
    s.cursor(cursor, click(t, 0.7));
}

// ---------------------------------------------------------------------------
// Organizing

fn focus(s: &mut Stage, t: f32) {
    let tile = r(170.0, 100.0, 110.0, 75.0);
    let k = ease(t, 0.8, 1.4) * (1.0 - ease(t, 2.7, 3.3));
    s.faded(1.0 - k, |s| {
        s.tile(r(30.0, 40.0, 120.0, 80.0), Art::Chat, 0.0);
        s.tile(r(250.0, 190.0, 120.0, 80.0), Art::Image, 0.0);
        s.tile(r(40.0, 180.0, 100.0, 80.0), Art::Video, 0.4);
    });
    let full = Rect::from_center_size(pos2(W / 2.0, H / 2.0), vec2(380.0, 259.0));
    let rect = lerp_rect(tile, full, k);
    s.tile(rect, Art::Window(TEAL), 0.0);
    if t > 0.4 && k < 0.05 {
        s.outline(rect, SELECT);
    }
    if t < 0.8 {
        let cursor = track(t, &[(0.0, pos2(330.0, 270.0)), (0.35, tile.center())]);
        s.cursor(cursor, click(t, 0.4));
    }
    let focus = s.slot(HotkeySlot::FocusCurrentTile);
    let exit = s.slot(HotkeySlot::ExitTileOrBrowser);
    s.key(&focus, show(t, 0.5, 1.3));
    s.key(&exit, show(t, 2.4, 3.2));
}

fn group(s: &mut Stage, t: f32) {
    let a0 = r(40.0, 80.0, 130.0, 90.0);
    let b0 = r(200.0, 120.0, 120.0, 110.0);
    let shift = vec2(25.0, -20.0) * ease(t, 1.6, 2.3);
    let a1 = a0.translate(shift);
    let b1 = b0.translate(shift);
    let frame = a1.union(b1);
    let gap = 6.0;
    let main = Rect::from_min_size(
        frame.min,
        vec2(frame.width() * 0.66 - gap / 2.0, frame.height()),
    );
    let side = Rect::from_min_max(
        pos2(main.max.x + gap, frame.min.y + frame.height() * 0.2),
        pos2(frame.max.x, frame.min.y + frame.height() * 0.8),
    );
    let k = ease(t, 2.8, 3.3);
    let a = lerp_rect(a1, main, k);
    let b = lerp_rect(b1, side, k);
    s.tile(a, Art::Video, t / LOOP);
    s.tile(b, Art::Chat, t);
    if t < 1.1 {
        if t >= 0.3 {
            s.outline(a, SELECT);
        }
        if t >= 0.7 {
            s.outline(b, SELECT);
        }
    } else {
        let alpha = ramp(t, 1.1, 1.3);
        s.faded(alpha, |s| {
            s.painter.rect_stroke(
                s.rect(frame.expand(5.0)),
                s.s(6.0),
                Stroke::new(s.s(1.5), ui_theme::GOLD.gamma_multiply(0.8)),
            );
            let bar = r(frame.min.x - 5.0, frame.min.y - 27.0, 92.0, 18.0);
            s.painter.rect(
                s.rect(bar),
                s.s(5.0),
                ui_theme::PANEL,
                Stroke::new(s.s(1.0), ui_theme::BORDER),
            );
            s.text(
                bar.left_center() + vec2(8.0, 0.0),
                Align2::LEFT_CENTER,
                "Group 1",
                9.5,
                ui_theme::TEXT,
            );
            s.text(
                bar.right_center() - vec2(12.0, 0.0),
                Align2::CENTER_CENTER,
                icon::LAYOUT,
                10.0,
                ui_theme::GOLD,
            );
        });
    }
    let cursor = if t < 1.6 {
        track(
            t,
            &[
                (0.0, pos2(330.0, 270.0)),
                (0.25, a0.center()),
                (0.6, b0.center()),
            ],
        )
    } else if t < 2.3 {
        b0.center() + shift
    } else {
        track(t, &[(2.3, b1.center()), (2.8, pos2(340.0, 270.0))])
    };
    s.cursor(cursor, click(t, 0.3).or(click(t, 0.7)));
    let group = s.slot(HotkeySlot::GroupTiles);
    let layout = s.slot(HotkeySlot::LayoutOneLarge);
    s.key(&group, show(t, 0.9, 1.6));
    s.key(&layout, show(t, 2.6, 3.4));
}

fn tile_center(s: &mut Stage, t: f32) {
    let tiles = [
        (r(20.0, 40.0, 120.0, 80.0), Art::Window(TEAL)),
        (r(150.0, 50.0, 80.0, 70.0), Art::Video),
        (r(30.0, 140.0, 100.0, 90.0), Art::Image),
        (r(145.0, 150.0, 90.0, 80.0), Art::Chat),
    ];
    let chosen = 2;
    for (i, (rect, art)) in tiles.iter().enumerate() {
        s.tile(*rect, *art, 0.0);
        if i == chosen && t >= 1.8 {
            let alpha = show(t, 1.8, LOOP);
            s.faded(alpha, |s| s.outline(*rect, INTERACT));
        }
    }
    let x = W + 10.0 - 175.0 * ease(t, 0.4, 0.9);
    let content = s.window_frame(r(x, 20.0, 160.0, 250.0), "Control Tile Center");
    let card = |i: usize| {
        let (col, row) = ((i % 2) as f32, (i / 2) as f32);
        r(
            content.min.x + 8.0 + col * 74.0,
            content.min.y + 8.0 + row * 74.0,
            68.0,
            66.0,
        )
    };
    for (i, (_, art)) in tiles.iter().enumerate() {
        let rect = card(i);
        s.fill(rect, 4.0, ui_theme::SURFACE);
        if i == chosen && t >= 1.8 {
            s.painter.rect_stroke(
                s.rect(rect),
                s.s(4.0),
                Stroke::new(s.s(1.2), ui_theme::GOLD),
            );
        }
        s.tile(r(rect.min.x + 4.0, rect.min.y + 4.0, 60.0, 40.0), *art, 0.0);
        s.fill(
            r(rect.min.x + 5.0, rect.min.y + 51.0, 40.0, 5.0),
            2.0,
            ui_theme::SECONDARY,
        );
    }
    let cursor = track(
        t,
        &[
            (0.0, pos2(220.0, 280.0)),
            (1.0, pos2(220.0, 280.0)),
            (1.6, card(chosen).center()),
        ],
    );
    s.cursor(cursor, click(t, 1.7));
    let label = s.slot(HotkeySlot::ToggleTileControl);
    s.key(&label, show(t, 0.1, 0.9));
}

fn pin(s: &mut Stage, t: f32) {
    let offset = vec2(-110.0, -30.0) * ease(t, 0.9, 2.8);
    s.grid(offset, 1.0);
    for (rect, art) in [
        (r(40.0, 60.0, 130.0, 90.0), Art::Image),
        (r(200.0, 150.0, 140.0, 95.0), Art::Window(VIOLET)),
        (r(50.0, 180.0, 110.0, 80.0), Art::Video),
        (r(370.0, 60.0, 120.0, 90.0), Art::Browser),
        (r(400.0, 200.0, 110.0, 80.0), Art::Game),
    ] {
        s.tile(view(rect, offset, 1.0), art, 0.0);
    }
    let pinned = r(276.0, 18.0, 110.0, 80.0);
    s.tile(pinned, Art::Chat, t);
    let badge = pinned.right_top() + vec2(-4.0, 4.0);
    s.painter
        .circle_filled(s.pt(badge), s.s(9.0), ui_theme::GOLD_DARK);
    s.text(
        badge,
        Align2::CENTER_CENTER,
        icon::PUSH_PIN,
        11.0,
        ui_theme::GOLD,
    );
    let cursor = pos2(200.0, 200.0) + offset;
    s.cursor(cursor, None);
    s.cursor_hint(cursor, icon::MOUSE_MIDDLE_CLICK, show(t, 0.7, 3.0));
}

fn pip(s: &mut Stage, t: f32) {
    s.tile(r(0.0, 0.0, W, H), Art::Game, t / LOOP * 2.0);
    let from = r(40.0, 66.0, 140.0, 95.0);
    let window = r(24.0, 24.0, 250.0, 190.0);
    let alpha = 1.0 - ease(t, 2.2, 2.7);
    s.faded(alpha, |s| {
        let content = s.window_frame(window, "Pluriview");
        s.fill(content.shrink(2.0), 4.0, CANVAS);
        s.tile(r(192.0, 70.0, 74.0, 130.0), Art::Chat, t);
        if t < 1.0 {
            s.tile(from, Art::Video, t / LOOP);
        } else {
            s.painter.rect_stroke(
                s.rect(from),
                s.s(5.0),
                Stroke::new(s.s(1.0), ui_theme::BORDER),
            );
            s.text(
                from.center(),
                Align2::CENTER_CENTER,
                icon::PICTURE_IN_PICTURE,
                18.0,
                ui_theme::SECONDARY,
            );
        }
    });
    if t >= 1.0 {
        let rect = lerp_rect(from, r(266.0, 188.0, 120.0, 82.0), ease(t, 1.0, 1.7));
        s.fill(
            rect.translate(vec2(3.0, 4.0)),
            5.0,
            Color32::from_black_alpha(110),
        );
        s.tile(rect, Art::Video, t / LOOP);
        s.painter.rect_stroke(
            s.rect(rect),
            s.s(5.0),
            Stroke::new(s.s(1.0), ui_theme::BORDER),
        );
    }
    let label = s.slot(HotkeySlot::PictureInPicture);
    s.key(&label, show(t, 0.4, 1.3));
}

// ---------------------------------------------------------------------------
// Capturing and sharing

fn screenshot(s: &mut Stage, t: f32) {
    let a = r(40.0, 70.0, 140.0, 100.0);
    let b = r(200.0, 90.0, 150.0, 110.0);
    let both = a.union(b);
    s.tile(a, Art::Window(TEAL), 0.0);
    s.tile(b, Art::Image, 0.0);
    s.outline(a, SELECT);
    s.outline(b, SELECT);
    if t >= 0.8 {
        let flash = 1.0 - ramp(t, 0.8, 1.2);
        s.fill(both, 5.0, Color32::WHITE.gamma_multiply(0.85 * flash));
    }
    let button = pos2(368.0, 26.0);
    let pulse = if (2.0..2.4).contains(&t) {
        3.0 * (1.0 - ramp(t, 2.0, 2.4))
    } else {
        0.0
    };
    s.painter
        .circle_filled(s.pt(button), s.s(13.0 + pulse), ui_theme::SURFACE);
    s.text(
        button,
        Align2::CENTER_CENTER,
        icon::CAMERA,
        14.0,
        ui_theme::GOLD,
    );
    if (1.0..2.05).contains(&t) {
        let shot = lerp_rect(
            both.expand(4.0),
            Rect::from_center_size(button, vec2(30.0, 20.0)),
            ease(t, 1.3, 2.0),
        );
        s.fill(shot, 3.0, Color32::WHITE);
        let inner = shot.shrink(shot.width() * 0.02 + 1.0);
        let place = |rect: Rect| {
            let map = |p: Pos2| {
                inner.min
                    + vec2(
                        (p.x - both.min.x) / both.width() * inner.width(),
                        (p.y - both.min.y) / both.height() * inner.height(),
                    )
            };
            Rect::from_min_max(map(rect.min), map(rect.max))
        };
        s.fill(inner, 2.0, CANVAS);
        s.tile(place(a), Art::Window(TEAL), 0.0);
        s.tile(place(b), Art::Image, 0.0);
    }
    let label = s.slot(HotkeySlot::ScreenshotTile);
    s.key(&label, show(t, 0.4, 1.1));
}

fn record(s: &mut Stage, t: f32) {
    let tile = r(70.0, 50.0, 260.0, 180.0);
    s.tile(tile, Art::Game, t / LOOP * 2.0);
    if t >= 0.7 {
        let pulse = 0.55 + 0.45 * (t * TAU * 0.8).sin().abs();
        s.painter.rect_stroke(
            s.rect(tile.expand(2.0)),
            s.s(6.0),
            Stroke::new(s.s(2.5), REC.gamma_multiply(pulse)),
        );
        let badge = r(tile.min.x + 8.0, tile.min.y + 8.0, 66.0, 18.0);
        s.fill(badge, 9.0, Color32::from_black_alpha(170));
        s.painter.circle_filled(
            s.pt(badge.left_center() + vec2(10.0, 0.0)),
            s.s(4.0),
            REC.gamma_multiply(pulse),
        );
        let seconds = (t - 0.7) as u32;
        s.text(
            badge.left_center() + vec2(18.0, 0.0),
            Align2::LEFT_CENTER,
            &format!("REC 0:{seconds:02}"),
            9.0,
            Color32::WHITE,
        );
    }
    let label = s.slot(HotkeySlot::RecordTile);
    s.key(&label, show(t, 0.3, 1.0));
}

fn screenshot_panel(s: &mut Stage, t: f32) {
    s.tile(r(30.0, 40.0, 150.0, 100.0), Art::Chat, 0.0);
    let x = W - 130.0 * ease(t, 0.3, 0.8);
    s.fill(r(x, 0.0, 130.0, H), 0.0, ui_theme::PANEL);
    s.fill(r(x, 0.0, 1.0, H), 0.0, ui_theme::BORDER);
    s.text(
        pos2(x + 10.0, 14.0),
        Align2::LEFT_CENTER,
        "Screenshots",
        10.0,
        ui_theme::TEXT,
    );
    let arts = [
        Art::Image,
        Art::Game,
        Art::Video,
        Art::Window(BLUE),
        Art::Image,
        Art::Chat,
    ];
    let thumb = |i: usize| {
        r(
            x + 8.0 + (i % 2) as f32 * 60.0,
            30.0 + (i / 2) as f32 * 50.0,
            54.0,
            42.0,
        )
    };
    for (i, art) in arts.into_iter().enumerate() {
        if i == 0 && (1.5..2.5).contains(&t) {
            s.fill(thumb(i), 4.0, ui_theme::SURFACE);
        } else {
            s.tile(thumb(i), art, 0.0);
        }
    }
    let drop = pos2(110.0, 210.0);
    let cursor = track(
        t,
        &[
            (0.0, pos2(200.0, 270.0)),
            (0.9, pos2(200.0, 270.0)),
            (1.3, thumb(0).center()),
            (1.5, thumb(0).center()),
            (2.4, drop),
            (2.9, drop),
            (3.3, pos2(250.0, 280.0)),
        ],
    );
    if (1.5..2.5).contains(&t) {
        let ghost = Rect::from_center_size(cursor, vec2(54.0, 42.0));
        s.faded(0.85, |s| s.tile(ghost, Art::Image, 0.0));
    }
    if t >= 2.5 {
        let tile = lerp_rect(
            Rect::from_center_size(drop, vec2(54.0, 42.0)),
            r(30.0, 160.0, 170.0, 115.0),
            ease(t, 2.5, 3.0),
        );
        s.tile(tile, Art::Image, 0.0);
    }
    s.cursor(cursor, None);
    let label = s.slot(HotkeySlot::ToggleScreenshotsPanel);
    s.key(&label, show(t, 0.05, 0.8));
}

fn stream_audio(s: &mut Stage, t: f32) {
    let tile = r(16.0, 90.0, 150.0, 110.0);
    s.tile(tile, Art::Video, t / LOOP);
    // A little equalizer shows the tile is playing sound.
    for i in 0..4 {
        let height = 6.0 + 10.0 * (0.5 + 0.5 * (t * 7.0 + i as f32 * 1.3).sin());
        s.fill(
            r(
                tile.max.x - 30.0 + i as f32 * 6.0,
                tile.min.y + 24.0 - height,
                4.0,
                height,
            ),
            1.0,
            ui_theme::GOLD,
        );
    }
    let monitor = r(196.0, 112.0, 82.0, 66.0);
    let target = r(306.0, 112.0, 82.0, 66.0);
    for (rect, glyph, label) in [
        (monitor, icon::SPEAKER_HIGH, "Stream Audio"),
        (target, icon::BROADCAST, "Discord / OBS"),
    ] {
        s.painter.rect(
            s.rect(rect),
            s.s(8.0),
            ui_theme::SURFACE,
            Stroke::new(s.s(1.0), ui_theme::BORDER),
        );
        s.text(
            rect.center() - vec2(0.0, 8.0),
            Align2::CENTER_CENTER,
            glyph,
            20.0,
            ui_theme::GOLD,
        );
        s.text(
            rect.center() + vec2(0.0, 18.0),
            Align2::CENTER_CENTER,
            label,
            8.5,
            ui_theme::TEXT,
        );
    }
    for (from, to) in [
        (tile.right_center(), monitor.left_center()),
        (monitor.right_center(), target.left_center()),
    ] {
        for i in 0..4 {
            let k = (t * 0.6 + i as f32 / 4.0).fract();
            let at = from + (to - from) * k;
            let alpha = (k * 4.0).min((1.0 - k) * 4.0).min(1.0);
            s.painter
                .circle_filled(s.pt(at), s.s(2.5), ui_theme::GOLD.gamma_multiply(alpha));
        }
    }
}

fn phone(s: &mut Stage, t: f32) {
    let moved = vec2(40.0, 70.0) * ease(t, 1.2, 2.2);
    let tiles = [
        (
            r(20.0, 40.0, 120.0, 80.0).translate(moved),
            Art::Window(TEAL),
        ),
        (r(160.0, 40.0, 100.0, 120.0), Art::Video),
        (r(30.0, 180.0, 90.0, 80.0), Art::Image),
    ];
    for (rect, art) in tiles {
        s.tile(rect, art, t / LOOP);
    }
    let phone = r(300.0, 50.0, 86.0, 176.0);
    s.painter.rect(
        s.rect(phone),
        s.s(12.0),
        Color32::from_rgb(8, 8, 10),
        Stroke::new(s.s(2.0), ui_theme::BORDER),
    );
    let screen = phone.shrink(6.0);
    s.fill(screen, 8.0, CANVAS);
    let world = r(0.0, 20.0, 280.0, 260.0);
    let k = (screen.width() / world.width()).min(screen.height() / world.height());
    let offset = screen.center() - world.center().to_vec2() * k;
    for (rect, art) in tiles {
        let mapped = Rect::from_min_max(
            offset + rect.min.to_vec2() * k,
            offset + rect.max.to_vec2() * k,
        );
        s.tile(mapped, art, t / LOOP);
    }
    let pulse = 0.4 + 0.6 * (t * TAU * 0.5).sin().abs();
    s.text(
        pos2(343.0, 32.0),
        Align2::CENTER_CENTER,
        icon::WIFI_HIGH,
        16.0,
        ui_theme::GOLD.gamma_multiply(pulse),
    );
    let grab = tiles[0].0.center();
    let cursor = track(t, &[(0.0, pos2(200.0, 270.0)), (1.1, pos2(80.0, 80.0))]);
    s.cursor(if t >= 1.2 { grab } else { cursor }, None);
}

// ---------------------------------------------------------------------------
// Making it yours

fn viewpoints(s: &mut Stage, t: f32) {
    let there = vec2(-330.0, -30.0);
    let k = ease(t, 1.0, 1.8) * (1.0 - ease(t, 2.6, 3.4));
    let offset = there * k;
    s.grid(offset, 1.0);
    for (rect, art) in [
        (r(50.0, 60.0, 130.0, 90.0), Art::Window(TEAL)),
        (r(200.0, 90.0, 120.0, 100.0), Art::Image),
        (r(80.0, 170.0, 110.0, 80.0), Art::Chat),
        (r(390.0, 70.0, 140.0, 100.0), Art::Video),
        (r(550.0, 100.0, 110.0, 110.0), Art::Game),
        (r(420.0, 190.0, 150.0, 70.0), Art::Browser),
    ] {
        s.tile(view(rect, offset, 1.0), art, t / LOOP);
    }
    s.bookmark(pos2(50.0, 30.0) + offset, "1");
    s.bookmark(pos2(390.0, 40.0) + offset, "2");
    let second = s.slot(HotkeySlot::CanvasView2);
    let first = s.slot(HotkeySlot::CanvasView1);
    s.key(&second, show(t, 0.5, 1.3));
    s.key(&first, show(t, 2.1, 2.9));
}

fn wallpaper(s: &mut Stage, t: f32) {
    s.grid(Vec2::ZERO, 1.0);
    let alpha = ease(t, 0.8, 1.6);
    s.faded(alpha, |s| {
        s.gradient(
            r(0.0, 0.0, W, H * 0.7),
            Color32::from_rgb(40, 30, 90),
            Color32::from_rgb(235, 130, 100),
        );
        s.painter.circle_filled(
            s.pt(pos2(300.0, 190.0)),
            s.s(40.0),
            Color32::from_rgb(255, 200, 120),
        );
        s.gradient(
            r(0.0, H * 0.7, W, H * 0.3),
            Color32::from_rgb(50, 30, 70),
            Color32::from_rgb(20, 14, 30),
        );
    });
    s.tile(r(30.0, 40.0, 150.0, 100.0), Art::Window(VIOLET), 0.0);
    s.tile(r(210.0, 30.0, 150.0, 110.0), Art::Chat, t);
    s.tile(r(112.0, 146.0, 114.0, 152.0), Art::Avatar, t / LOOP);
}

/// Overlay mode over 8 s: a small always-on-top Pluriview window floats
/// over a game. Mira can be dragged until Click Through is turned on from the
/// tray; after that neither she nor the tile behind her reacts, and a
/// cut-away shows each click landing in the game underneath instead.
fn overlay(s: &mut Stage, t: f32) {
    let screen = r(0.0, 0.0, W, H);
    s.tile(screen, Art::Game, t / LOOP);
    let through = t >= 3.75;

    // Pluriview, pinned on top.
    let window = r(192.0, 22.0, 196.0, 240.0);
    let content = s.window_frame(window, "Pluriview");
    s.text(
        pos2(window.max.x - 12.0, window.min.y + 10.0),
        Align2::CENTER_CENTER,
        icon::PUSH_PIN,
        10.0,
        ui_theme::GOLD,
    );
    let canvas = content.shrink(2.0);
    s.fill(canvas, 5.0, CANVAS);
    let full = s.painter.clone();
    s.painter = full.with_clip_rect(s.rect(canvas));
    s.grid(Vec2::ZERO, 1.0);
    let chat = r(202.0, 52.0, 84.0, 128.0);
    s.tile(chat, Art::Chat, t);
    let home = r(276.0, 112.0, 102.0, 136.0);
    let moved = home.translate(vec2(-46.0, 0.0));
    let her = track_rect(t, &[(1.15, home), (2.05, moved)]);
    s.tile(her, Art::Avatar, t / LOOP);
    if (1.1..2.2).contains(&t) {
        s.outline(her, SELECT);
    }
    s.painter = full;

    // The taskbar and Pluriview's tray icon.
    let bar = r(0.0, H - 18.0, W, 18.0);
    s.fill(bar, 0.0, Color32::from_rgb(24, 25, 30));
    let tray = pos2(W - 16.0, H - 9.0);
    s.fill(
        Rect::from_center_size(tray, vec2(12.0, 12.0)),
        3.0,
        ui_theme::GOLD,
    );
    s.text(tray, Align2::CENTER_CENTER, "P", 8.0, CANVAS);

    // The tray menu, Click Through ticked at 3.75 s.
    if (3.0..4.1).contains(&t) {
        let alpha = show(t, 3.0, 4.1);
        s.faded(alpha, |s| {
            let tick = |on: bool, label: &str| {
                if on {
                    format!("{}  {label}", icon::CHECK)
                } else {
                    format!("      {label}")
                }
            };
            let items = [
                tick(false, "Show"),
                tick(false, "Hide"),
                tick(true, "Always on Top"),
                tick(through, "Click Through"),
                tick(false, "Quit"),
            ];
            let items: Vec<&str> = items.iter().map(String::as_str).collect();
            let lit = (t >= 3.45).then_some(3);
            s.menu(pos2(W - 128.0, H - 118.0), &items, lit);
        });
    }

    // The pointer drags Mira, opens the tray menu, then tries again.
    let menu_row = pos2(W - 70.0, H - 59.0);
    let cursor = track(
        t,
        &[
            (0.3, pos2(120.0, 120.0)),
            (1.0, home.center()),
            (1.15, home.center()),
            (2.05, moved.center()),
            (2.3, moved.center()),
            (2.9, tray + vec2(-3.0, -3.0)),
            (3.15, tray + vec2(-3.0, -3.0)),
            (3.6, menu_row),
            (3.8, menu_row),
            (4.6, moved.center()),
            (4.8, moved.center()),
            (5.5, moved.center() + vec2(-60.0, -16.0)),
            (6.0, moved.center() + vec2(-60.0, -16.0)),
            (6.5, pos2(222.0, 70.0)),
        ],
    );
    let presses = [1.1, 3.0, 3.7, 4.8, 6.6];
    let pressed = presses.iter().find_map(|&at| click(t, at));

    // After Click Through, a cut-away under the pointer shows where the
    // clicks go: the game behind the window.
    let peek = show(t, 4.75, 5.75).max(show(t, 6.55, 7.4));
    if peek > 0.0 {
        let lens = Rect::from_center_size(cursor, vec2(52.0, 52.0));
        s.faded(peek, |s| {
            s.art(screen, lens, Art::Game, t / LOOP);
            s.painter.rect_stroke(
                s.rect(lens),
                s.s(5.0),
                Stroke::new(s.s(1.4), ui_theme::GOLD),
            );
        });
    }
    if (1.1..2.1).contains(&t) {
        s.cursor_hint(cursor, icon::HAND_GRABBING, show(t, 1.1, 2.1));
    }
    s.cursor(cursor, pressed);

    // What state the window is in.
    let chip = r(10.0, 10.0, 132.0, 22.0);
    s.painter.rect(
        s.rect(chip),
        s.s(6.0),
        ui_theme::PANEL,
        Stroke::new(
            s.s(1.0),
            if through {
                ui_theme::GOLD
            } else {
                ui_theme::BORDER
            },
        ),
    );
    s.text(
        chip.left_center() + vec2(12.0, 0.0),
        Align2::CENTER_CENTER,
        icon::CURSOR_CLICK,
        11.0,
        if through {
            ui_theme::GOLD
        } else {
            ui_theme::SECONDARY
        },
    );
    s.text(
        chip.left_center() + vec2(24.0, 0.0),
        Align2::LEFT_CENTER,
        if through {
            "Click Through: on"
        } else {
            "Click Through: off"
        },
        9.5,
        ui_theme::TEXT,
    );
    let note = show(t, 4.9, 7.8);
    s.faded(note, |s| {
        let rect = r(10.0, 36.0, 158.0, 22.0);
        s.fill(rect, 6.0, Color32::from_black_alpha(150));
        s.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "Clicks go to the app behind",
            9.5,
            ui_theme::TEXT,
        );
    });
}

fn hide_ui(s: &mut Stage, t: f32) {
    s.tile(r(130.0, 50.0, 130.0, 90.0), Art::Window(TEAL), 0.0);
    s.tile(r(272.0, 70.0, 110.0, 100.0), Art::Video, t / LOOP);
    s.tile(r(150.0, 160.0, 180.0, 110.0), Art::Image, 0.0);
    let hidden = ease(t, 1.0, 1.4) * (1.0 - ease(t, 2.8, 3.2));
    let bar = r(0.0, -26.0 * hidden, W, 24.0);
    let panel = r(-102.0 * hidden, 24.0 - 24.0 * hidden, 100.0, H);
    s.fill(panel, 0.0, ui_theme::PANEL);
    s.fill(
        r(panel.max.x - 1.0, panel.min.y, 1.0, H),
        0.0,
        ui_theme::BORDER,
    );
    for (i, color) in PALETTE.iter().enumerate() {
        let y = panel.min.y + 16.0 + i as f32 * 30.0;
        s.fill(r(panel.min.x + 8.0, y, 14.0, 14.0), 3.0, *color);
        s.fill(
            r(panel.min.x + 28.0, y + 4.0, 56.0, 5.0),
            2.0,
            ui_theme::SECONDARY,
        );
    }
    s.fill(bar, 0.0, ui_theme::PANEL);
    for (i, width) in [50.0, 28.0, 30.0, 26.0].into_iter().enumerate() {
        s.fill(
            r(12.0 + i as f32 * 62.0, bar.min.y + 10.0, width, 5.0),
            2.0,
            ui_theme::SECONDARY,
        );
    }
    let label = s.slot(HotkeySlot::ToggleCanvasOnly);
    s.key(&label, show(t, 0.6, 1.4).max(show(t, 2.4, 3.2)));
}

fn shortcuts(s: &mut Stage, t: f32) {
    s.faded(0.5, |s| {
        s.tile(r(20.0, 30.0, 120.0, 90.0), Art::Window(BLUE), 0.0);
        s.tile(r(270.0, 170.0, 110.0, 90.0), Art::Image, 0.0);
    });
    let rows = [
        ("Window Picker", HotkeySlot::ToggleWindowPicker),
        ("Select all", HotkeySlot::SelectAll),
        ("Focus tile", HotkeySlot::FocusCurrentTile),
        ("Group", HotkeySlot::GroupTiles),
        ("Picture in Picture", HotkeySlot::PictureInPicture),
        ("Screenshot", HotkeySlot::ScreenshotTile),
        ("Record", HotkeySlot::RecordTile),
        ("Hide interface", HotkeySlot::ToggleCanvasOnly),
    ];
    let k = ease(t, 0.6, 0.9);
    let alpha = ramp(t, 0.6, 0.8);
    let lit = ((t - 1.2) / 0.32).floor();
    s.faded(alpha, |s| {
        let dialog = scale_about(r(90.0, 22.0, 220.0, 232.0), 0.9 + 0.1 * k);
        let content = s.window_frame(dialog, "Keyboard Shortcuts");
        for (i, (name, slot)) in rows.iter().enumerate() {
            let row = r(
                content.min.x + 6.0,
                content.min.y + 4.0 + i as f32 * 25.0,
                content.width() - 12.0,
                22.0,
            );
            if lit == i as f32 {
                s.fill(row, 4.0, ui_theme::GOLD_DARK);
            }
            s.text(
                row.left_center() + vec2(6.0, 0.0),
                Align2::LEFT_CENTER,
                name,
                9.5,
                ui_theme::TEXT,
            );
            s.text(
                row.right_center() - vec2(6.0, 0.0),
                Align2::RIGHT_CENTER,
                &s.slot(*slot),
                9.0,
                ui_theme::GOLD,
            );
        }
    });
    let label = s.slot(HotkeySlot::ShowShortcutHelp);
    s.key(&label, show(t, 0.15, 0.9));
}
