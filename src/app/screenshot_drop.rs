use super::*;
use std::path::PathBuf;

/// Grid cells match the image tile box, so tiles never overlap.
const CELL: Vec2 = Vec2::new(640.0, 480.0);
const GAP: f32 = 24.0;
/// One drop adds at most this many tiles; every image tile keeps its full
/// decoded frames in memory.
pub(super) const MAX_DROPPED_SCREENSHOTS: usize = 48;
const LANDING_SECS: f32 = 0.45;
/// Each extra screenshot leaves slightly later, so a batch streams out.
const LANDING_STAGGER_SECS: f32 = 0.04;

/// A thumbnail flying from the panel (or the drag ghost) to its new tile.
pub(super) struct Landing {
    id: PreviewId,
    from: egui::Rect,
    texture: egui::TextureHandle,
    started: Instant,
}

/// Top-left positions for tiles of the given sizes, in a near-square grid
/// centered on `center`. Rows are as tall as their tallest tile.
fn drop_positions(sizes: &[Vec2], center: Pos2) -> Vec<Pos2> {
    if sizes.is_empty() {
        return Vec::new();
    }
    let columns = (sizes.len() as f32).sqrt().ceil() as usize;
    let row_heights: Vec<f32> = sizes
        .chunks(columns)
        .map(|row| row.iter().map(|size| size.y).fold(0.0, f32::max))
        .collect();
    let used_columns = columns.min(sizes.len());
    let width = if sizes.len() == 1 {
        sizes[0].x
    } else {
        used_columns as f32 * CELL.x + (used_columns - 1) as f32 * GAP
    };
    let height = row_heights.iter().sum::<f32>() + row_heights.len().saturating_sub(1) as f32 * GAP;
    let origin = center - Vec2::new(width, height) / 2.0;
    let mut positions = Vec::with_capacity(sizes.len());
    let mut y = origin.y;
    for (row, row_height) in row_heights.iter().enumerate() {
        for column in 0..columns {
            if row * columns + column >= sizes.len() {
                break;
            }
            positions.push(Pos2::new(origin.x + column as f32 * (CELL.x + GAP), y));
        }
        y += row_height + GAP;
    }
    positions
}

fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

impl PluriviewApp {
    /// Add screenshots as image tiles around `screen_point`. Each tile flies
    /// in from its `sources` rect (panel card or drag ghost) before it fades in.
    pub(super) fn add_screenshots_to_canvas(
        &mut self,
        paths: &[PathBuf],
        screen_point: Pos2,
        sources: &[egui::Rect],
        ctx: &egui::Context,
    ) {
        let Some(canvas_rect) = self.canvas.last_screen_rect else {
            return;
        };
        let Some(root) = self.screenshots_dir() else {
            return;
        };
        let paths: Vec<&PathBuf> = paths
            .iter()
            .filter(|path| crate::screenshot_library::shot_folder(&root, path).is_some())
            .take(MAX_DROPPED_SCREENSHOTS)
            .collect();
        if paths.is_empty() {
            return;
        }
        // PNG headers give the proportions up front, so the grid is final
        // before any image decodes.
        let sizes: Vec<Vec2> = paths
            .iter()
            .map(|path| match image::image_dimensions(path) {
                Ok((width, height)) => media_tile_size(width, height),
                Err(_) => CELL,
            })
            .collect();
        let center = self.canvas.screen_to_canvas(screen_point, canvas_rect);
        let positions = drop_positions(&sizes, center);

        let mut added = Vec::new();
        let mut errors = Vec::new();
        let now = Instant::now();
        for (index, ((path, size), position)) in paths.iter().zip(&sizes).zip(positions).enumerate()
        {
            match self.import_media_tile_sized(path, position, *size) {
                Ok(id) => {
                    let delay = index as f32 * LANDING_STAGGER_SECS;
                    let texture = self.screenshots.panel.texture_handle(path);
                    if let (Some(texture), Some(preview)) =
                        (texture, self.preview_manager.get_mut(id))
                    {
                        // The tile fades in as its thumbnail arrives.
                        preview.created_at =
                            now + Duration::from_secs_f32(delay + LANDING_SECS * 0.75);
                        let from = sources
                            .get(index)
                            .or(sources.last())
                            .copied()
                            .unwrap_or_else(|| {
                                egui::Rect::from_center_size(screen_point, Vec2::splat(40.0))
                            });
                        self.screenshots.landings.push(Landing {
                            id,
                            from,
                            texture,
                            started: now + Duration::from_secs_f32(delay),
                        });
                    }
                    added.push(id);
                }
                Err(error) => errors.push(format!("{}: {error}", path.display())),
            }
        }
        if !added.is_empty() {
            self.canvas.exit_focus();
            self.canvas.selection = added;
        }
        if !errors.is_empty() {
            self.media_error = Some(errors.join("\n\n"));
        }
        ctx.request_repaint();
    }

    /// Highlight the canvas while screenshots are dragged over it.
    pub(super) fn screenshot_drop_hint_ui(&self, ctx: &egui::Context, count: usize) {
        let (Some(rect), Some(pointer)) = (self.canvas.last_screen_rect, ctx.pointer_latest_pos())
        else {
            return;
        };
        if !rect.contains(pointer) {
            return;
        }
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("screenshot_canvas_drop"),
        ));
        painter.rect_filled(rect, 0.0, crate::ui_theme::GOLD.gamma_multiply(0.06));
        painter.rect_stroke(
            rect.shrink(6.0),
            10.0,
            egui::Stroke::new(2.0, crate::ui_theme::GOLD.gamma_multiply(0.8)),
        );
        let label = if count == 1 {
            "Drop to add the screenshot to the canvas".to_owned()
        } else if count > MAX_DROPPED_SCREENSHOTS {
            format!("Drop to add the first {MAX_DROPPED_SCREENSHOTS} of {count} screenshots")
        } else {
            format!("Drop to add {count} screenshots to the canvas")
        };
        let text_pos = egui::pos2(rect.center().x, rect.top() + 34.0);
        let galley = painter.layout_no_wrap(
            label,
            egui::FontId::proportional(15.0),
            crate::ui_theme::TEXT,
        );
        let backing =
            egui::Rect::from_center_size(text_pos, galley.size() + egui::vec2(24.0, 12.0));
        painter.rect_filled(backing, 8.0, egui::Color32::from_rgb(24, 24, 28));
        painter.galley(
            text_pos - galley.size() / 2.0,
            galley,
            crate::ui_theme::TEXT,
        );
    }

    /// Draw thumbnails travelling to their new tiles. Targets follow the
    /// live tile rect, so panning mid-flight still lands correctly.
    pub(super) fn screenshot_landings_ui(&mut self, ctx: &egui::Context) {
        if self.screenshots.landings.is_empty() {
            return;
        }
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("screenshot_landings"),
        ));
        let now = Instant::now();
        let canvas = &self.canvas;
        let previews = &self.preview_manager;
        self.screenshots.landings.retain(|landing| {
            let Some(target) = canvas.tile_screen_rect(landing.id, previews) else {
                return false;
            };
            if now < landing.started {
                return true;
            }
            let t = (now - landing.started).as_secs_f32() / LANDING_SECS;
            if t >= 1.0 {
                return false;
            }
            let eased = ease_out_cubic(t);
            let size = landing.texture.size_vec2();
            let fit = (target.width() / size.x).min(target.height() / size.y);
            let target = egui::Rect::from_center_size(target.center(), size * fit);
            let rect = egui::Rect::from_min_max(
                landing.from.min + (target.min - landing.from.min) * eased,
                landing.from.max + (target.max - landing.from.max) * eased,
            );
            // Hand over to the tile's own fade-in during the last quarter.
            let alpha = ((1.0 - t) / 0.25).clamp(0.0, 1.0);
            painter.rect_filled(
                rect.translate(egui::vec2(0.0, 6.0)).expand(2.0),
                8.0,
                egui::Color32::from_black_alpha((70.0 * alpha) as u8),
            );
            painter.image(
                landing.texture.id(),
                rect,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE.gamma_multiply(alpha),
            );
            painter.rect_stroke(
                rect,
                6.0,
                egui::Stroke::new(1.5, crate::ui_theme::GOLD.gamma_multiply(alpha)),
            );
            true
        });
        ctx.request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_screenshot_is_centered_on_the_drop_point() {
        let positions = drop_positions(&[Vec2::new(640.0, 360.0)], Pos2::new(1000.0, 500.0));
        assert_eq!(positions, vec![Pos2::new(680.0, 320.0)]);
    }

    #[test]
    fn several_screenshots_form_a_grid_without_overlaps() {
        let sizes = vec![
            Vec2::new(640.0, 360.0),
            Vec2::new(360.0, 480.0),
            Vec2::new(640.0, 360.0),
            Vec2::new(640.0, 400.0),
            Vec2::new(640.0, 360.0),
        ];
        let positions = drop_positions(&sizes, Pos2::ZERO);
        assert_eq!(positions.len(), 5);
        // Three columns, two rows; the first row is as tall as its tallest tile.
        assert_eq!(positions[1].x - positions[0].x, CELL.x + GAP);
        assert_eq!(positions[3].y - positions[0].y, 480.0 + GAP);
        assert_eq!(positions[3].x, positions[0].x);
        for (index, (a, size_a)) in positions.iter().zip(&sizes).enumerate() {
            let a = egui::Rect::from_min_size(*a, *size_a);
            for (b, size_b) in positions.iter().zip(&sizes).skip(index + 1) {
                assert!(!a.intersects(egui::Rect::from_min_size(*b, *size_b)));
            }
        }
        // The grid is centered on the drop point.
        let right = positions[2].x + CELL.x;
        assert!((positions[0].x + right).abs() < 0.01);
    }
}
