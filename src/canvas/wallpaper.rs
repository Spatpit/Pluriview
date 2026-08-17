use crate::media::MediaFrame;
use crate::persistence::WallpaperLayout;
use crate::preview::PreviewId;
use eframe::egui::{self, Color32, Pos2, Rect, TextureHandle};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Reserved video-session id. Tile ids start at 1, so this never collides.
pub const WALLPAPER_VIDEO_ID: PreviewId = PreviewId(0);

/// Persistent source for the canvas wallpaper.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WallpaperSource {
    Image { managed_path: String },
    Video { path: PathBuf },
}

/// Screen-space background that fills the canvas without following pan/zoom.
#[derive(Clone)]
pub struct CanvasWallpaper {
    pub source: WallpaperSource,
    frames: Vec<MediaFrame>,
    frame_index: usize,
    frame_dirty: bool,
    frame_started: Instant,
    texture: Option<TextureHandle>,
    #[cfg(windows)]
    pub video_renderer: Option<std::sync::Arc<crate::libmpv::VideoRenderer>>,
    pub error: Option<String>,
}

impl CanvasWallpaper {
    pub fn from_image(managed_path: String, frames: Vec<MediaFrame>) -> Self {
        Self {
            source: WallpaperSource::Image { managed_path },
            frames,
            frame_index: 0,
            frame_dirty: true,
            frame_started: Instant::now(),
            texture: None,
            #[cfg(windows)]
            video_renderer: None,
            error: None,
        }
    }

    pub fn from_video(path: PathBuf) -> Self {
        Self {
            source: WallpaperSource::Video { path },
            frames: Vec::new(),
            frame_index: 0,
            frame_dirty: false,
            frame_started: Instant::now(),
            texture: None,
            #[cfg(windows)]
            video_renderer: None,
            error: None,
        }
    }

    pub fn to_layout(&self) -> WallpaperLayout {
        match &self.source {
            WallpaperSource::Image { managed_path } => WallpaperLayout::Image {
                path: managed_path.clone(),
            },
            WallpaperSource::Video { path } => WallpaperLayout::Video { path: path.clone() },
        }
    }

    pub fn paint(&mut self, painter: &egui::Painter, canvas_rect: Rect, ctx: &egui::Context) {
        #[cfg(windows)]
        if matches!(self.source, WallpaperSource::Video { .. }) {
            if let Some(renderer) = self.video_renderer.clone() {
                painter.add(egui::PaintCallback {
                    rect: canvas_rect,
                    callback: std::sync::Arc::new(eframe::egui_glow::CallbackFn::new(
                        move |info, painter| {
                            renderer.paint(info, painter.gl());
                        },
                    )),
                });
                if self.error.is_some() {
                    self.paint_video_status(painter, canvas_rect);
                }
            } else {
                self.paint_video_status(painter, canvas_rect);
                ctx.request_repaint();
            }
            return;
        }

        let uv = self.frames.get(self.frame_index).map_or_else(
            || Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            |frame| cover_uv(canvas_rect, frame.width as f32, frame.height as f32),
        );
        if let Some(texture) = self.texture(ctx) {
            painter.image(texture.id(), canvas_rect, uv, Color32::WHITE);
        }
    }

    fn paint_video_status(&self, painter: &egui::Painter, canvas_rect: Rect) {
        let message = self.error.as_deref().unwrap_or("Starting video wallpaper…");
        painter.text(
            canvas_rect.center(),
            egui::Align2::CENTER_CENTER,
            message,
            egui::FontId::proportional(16.0),
            Color32::from_rgb(160, 160, 168),
        );
    }

    fn texture(&mut self, ctx: &egui::Context) -> Option<&TextureHandle> {
        self.advance_animation(ctx);
        if self.frame_dirty {
            self.frame_dirty = false;
            if let Some(frame) = self.frames.get(self.frame_index) {
                let image = egui::ColorImage::from_rgba_unmultiplied(
                    [frame.width as usize, frame.height as usize],
                    &frame.rgba,
                );
                if let Some(texture) = self.texture.as_mut() {
                    texture.set(image, egui::TextureOptions::LINEAR);
                } else {
                    self.texture = Some(ctx.load_texture(
                        "canvas_wallpaper",
                        image,
                        egui::TextureOptions::LINEAR,
                    ));
                }
            }
        }
        self.texture.as_ref()
    }

    fn advance_animation(&mut self, ctx: &egui::Context) {
        if self.frames.len() <= 1 {
            return;
        }

        let mut elapsed = self.frame_started.elapsed();
        let mut advanced = false;
        for _ in 0..self.frames.len() {
            let delay = self.frames[self.frame_index].duration;
            if elapsed < delay {
                break;
            }
            elapsed = elapsed.saturating_sub(delay);
            self.frame_index = (self.frame_index + 1) % self.frames.len();
            self.frame_started = Instant::now() - elapsed;
            advanced = true;
        }

        if advanced {
            self.frame_dirty = true;
        }

        let delay = self.frames[self.frame_index].duration;
        let remaining = delay.saturating_sub(self.frame_started.elapsed());
        ctx.request_repaint_after(remaining.max(Duration::from_millis(1)));
    }
}

/// UV crop that fills `viewport` the way CSS `background-size: cover` does.
pub fn cover_uv(viewport: Rect, content_width: f32, content_height: f32) -> Rect {
    let viewport_width = viewport.width().max(1.0);
    let viewport_height = viewport.height().max(1.0);
    let content_width = content_width.max(1.0);
    let content_height = content_height.max(1.0);
    let viewport_aspect = viewport_width / viewport_height;
    let content_aspect = content_width / content_height;

    if content_aspect > viewport_aspect {
        let visible = viewport_aspect / content_aspect;
        let crop = (1.0 - visible) * 0.5;
        Rect::from_min_max(Pos2::new(crop, 0.0), Pos2::new(1.0 - crop, 1.0))
    } else if content_aspect < viewport_aspect {
        let visible = content_aspect / viewport_aspect;
        let crop = (1.0 - visible) * 0.5;
        Rect::from_min_max(Pos2::new(0.0, crop), Pos2::new(1.0, 1.0 - crop))
    } else {
        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0))
    }
}

#[cfg(test)]
mod tests {
    use super::cover_uv;
    use eframe::egui::{Pos2, Rect, Vec2};

    #[test]
    fn wide_content_crops_the_sides() {
        let viewport = Rect::from_min_size(Pos2::ZERO, Vec2::splat(100.0));
        let uv = cover_uv(viewport, 200.0, 100.0);
        assert!((uv.min.x - 0.25).abs() < f32::EPSILON);
        assert!((uv.max.x - 0.75).abs() < f32::EPSILON);
        assert_eq!(uv.min.y, 0.0);
        assert_eq!(uv.max.y, 1.0);
    }

    #[test]
    fn tall_content_crops_the_top_and_bottom() {
        let viewport = Rect::from_min_size(Pos2::ZERO, Vec2::splat(100.0));
        let uv = cover_uv(viewport, 100.0, 200.0);
        assert_eq!(uv.min.x, 0.0);
        assert_eq!(uv.max.x, 1.0);
        assert!((uv.min.y - 0.25).abs() < f32::EPSILON);
        assert!((uv.max.y - 0.75).abs() < f32::EPSILON);
    }

    #[test]
    fn matching_aspect_uses_the_full_image() {
        let viewport = Rect::from_min_size(Pos2::ZERO, Vec2::new(160.0, 90.0));
        let uv = cover_uv(viewport, 1920.0, 1080.0);
        assert_eq!(uv.min, Pos2::ZERO);
        assert_eq!(uv.max, Pos2::new(1.0, 1.0));
    }
}
