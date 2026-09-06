use super::animation::{AnimationState, CameraTransition, DragTracker};
use super::wallpaper::CanvasWallpaper;
use crate::capture::{capture_lod_factor, window_capture_target, CaptureCoordinator};
use crate::preview::{
    compact_title, BrowserTileStatus, FpsPreset, Preview, PreviewId, PreviewManager,
    RemovedPreviewInfo, VideoPlaybackState, VideoTileStatus, ViewportPin,
};
#[cfg(debug_assertions)]
use crate::privacy;
use eframe::egui::{self, Color32, CursorIcon, Pos2, Rect, Sense, Stroke, Vec2};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// How long the "Removed '...' · Undo" toast stays on screen.
const UNDO_TOAST_SECS: f32 = 4.0;
/// Screen-space hit target shared by resize interaction and marquee exclusion.
const RESIZE_HANDLE_HIT_SIZE: f32 = 14.0;
/// Playlist chrome is authored in canvas pixels so zoom keeps the same layout.
const PLAYLIST_CORNER: f32 = 10.0;
const PLAYLIST_HEADER_HEIGHT: f32 = 52.0;
const PLAYLIST_TOOLBAR_HEIGHT: f32 = 36.0;
const PLAYLIST_ROW_HEIGHT: f32 = 60.0;

/// A short pan away should pause frame processing, but should not destroy and
/// recreate Windows Graphics Capture. Long-off-screen captures release their
/// sessions while their independent process-audio monitors remain alive.
const CAPTURE_HIBERNATE_AFTER: Duration = Duration::from_secs(30);
/// Screen-space hysteresis around the viewport. A tile just beyond an edge is
/// cheap to return to and should not start the hibernation timer.
const CAPTURE_HIBERNATE_MARGIN_POINTS: f32 = 128.0;
/// A source that has not delivered its first frame should remain visibly in a
/// connecting state without forcing the entire canvas into an uncapped loop.
const WINDOW_CONNECTING_REPAINT_INTERVAL: Duration = Duration::from_millis(100);
/// Browser overlays may stay transparent until their external data arrives.
/// Their loading card needs motion, but not an uncapped canvas repaint loop.
const TRANSPARENT_BROWSER_REPAINT_INTERVAL: Duration = Duration::from_millis(100);

#[cfg(windows)]
use windows::Win32::Foundation::HWND;
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::{SetForegroundWindow, ShowWindow, SW_RESTORE};

/// Represents the current drag operation
#[derive(Clone, Debug)]
pub enum DragState {
    /// Resizing a preview. `aspect_ratio` is `None` when the tile may change
    /// width and height independently, such as a folder playlist.
    Resizing {
        id: PreviewId,
        handle: ResizeHandle,
        start_rect: Rect,
        start_mouse: Pos2,
        /// Aspect ratio to maintain during resize (width/height)
        aspect_ratio: Option<f32>,
        /// Pinned Spout tiles resize directly in viewport coordinates.
        screen_space: bool,
    },
    /// Cropping a preview (Alt+drag to adjust UV coordinates)
    Cropping {
        id: PreviewId,
        handle: ResizeHandle,
        start_mouse: Pos2,
        /// Tile rectangle when the crop began. Canvas coordinates normally;
        /// viewport coordinates for pinned tiles.
        start_rect: Rect,
        screen_space: bool,
        /// Starting crop UV coordinates (min_u, min_v, max_u, max_v)
        start_crop_uv: (f32, f32, f32, f32),
    },
}

#[cfg(test)]
mod tests {
    use super::{
        apply_crop, apply_resize, browser_control_colors, capture_resolution_badge_rect,
        format_time, live_capture_display_size, media_placeholder_content,
        native_capture_canvas_size, pixel_aligned_rect, playlist_first_row_center,
        rect_for_crop_change, stream_audio_badge_rect, submenu_should_open_left,
        video_placeholder_content, video_time_style, video_volume_popover_rect,
        video_volume_slider_rect, window_capture_placeholder_content, window_capture_target,
        BrowserAction, CanvasState, DragState, PlaylistAction, ResizeHandle, TileActivityAction,
        VideoAction,
    };
    use crate::capture::CaptureCoordinator;
    use crate::playlist::FolderPlaylist;
    use crate::preview::{FpsPreset, PreviewId, PreviewManager, VideoSource, VideoTileStatus};
    use eframe::egui::{
        self, CentralPanel, Context, CursorIcon, Event, Id, Modifiers, MouseWheelUnit, Order,
        PointerButton, Pos2, RawInput, Rect, Sense, Shape, Vec2,
    };
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    #[test]
    fn canvas_screen_rect_starts_empty() {
        assert!(CanvasState::default().last_screen_rect.is_none());
    }

    #[test]
    fn submenu_opens_left_only_when_the_right_side_is_too_narrow() {
        assert!(submenu_should_open_left(1850.0, 1920.0, 280.0));
        assert!(!submenu_should_open_left(800.0, 1920.0, 280.0));
    }

    #[test]
    fn saved_view_transition_finishes_with_target_center_and_zoom() {
        let mut canvas = CanvasState {
            pan: Vec2::new(-120.0, 40.0),
            zoom: 0.75,
            ..Default::default()
        };
        let viewport = Rect::from_min_size(Pos2::new(50.0, 80.0), Vec2::new(1200.0, 700.0));
        let target_center = Pos2::new(1600.0, -420.0);

        canvas.start_camera_transition(target_center, 2.25, viewport, 10.0);
        canvas.update_camera_transition(viewport, 11.0);

        assert!(canvas.camera_center(viewport).distance(target_center) < 0.001);
        assert!((canvas.zoom - 2.25).abs() < 0.0001);
        assert!(canvas.animation.camera_transition.is_none());
    }

    #[test]
    fn retargeting_a_saved_view_transition_is_continuous() {
        let mut canvas = CanvasState::default();
        let viewport = Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 600.0));
        canvas.start_camera_transition(Pos2::new(2000.0, 500.0), 2.0, viewport, 3.0);
        canvas.update_camera_transition(viewport, 3.2);
        let center_before = canvas.camera_center(viewport);
        let zoom_before = canvas.zoom;

        canvas.start_camera_transition(Pos2::new(-800.0, 900.0), 0.4, viewport, 3.2);
        canvas.update_camera_transition(viewport, 3.2);

        assert!(canvas.camera_center(viewport).distance(center_before) < 0.001);
        assert!((canvas.zoom - zoom_before).abs() < 0.0001);
    }

    #[test]
    fn double_click_target_starts_empty() {
        assert!(CanvasState::default().last_double_clicked.is_none());
    }

    #[test]
    fn browser_add_request_starts_empty() {
        assert!(CanvasState::default().pending_browser_add.is_none());
    }

    #[test]
    fn browser_history_buttons_keep_a_blue_accent_without_direct_hover() {
        let (idle_background, idle_icon) =
            browser_control_colors(BrowserAction::Back, false, false);
        let (hovered_background, hovered_icon) =
            browser_control_colors(BrowserAction::Forward, true, false);
        let (reload_background, reload_icon) =
            browser_control_colors(BrowserAction::Reload, false, false);

        assert!(idle_background.is_some());
        assert!(hovered_background.unwrap().a() > idle_background.unwrap().a());
        assert_ne!(idle_icon, reload_icon);
        assert_ne!(hovered_icon, reload_icon);
        assert!(reload_background.is_none());
    }

    #[test]
    fn video_action_queue_starts_empty() {
        let canvas = CanvasState::default();
        assert!(canvas.pending_video_actions.is_empty());
    }

    #[test]
    fn video_volume_popover_fits_the_tile_and_stays_above_the_mute_control() {
        let tile = Rect::from_min_size(Pos2::new(100.0, 80.0), Vec2::new(400.0, 240.0));
        let mute = Rect::from_min_size(Pos2::new(466.0, 287.0), Vec2::splat(26.0));

        let popover = video_volume_popover_rect(tile, mute);

        assert!(tile.contains(popover.min));
        assert!(tile.contains(popover.max));
        assert_eq!(popover.width(), 36.0);
        assert!((popover.center().x - mute.center().x).abs() <= 1.0);
        assert!((tile.right() - popover.right() - 3.0).abs() < f32::EPSILON);
        assert!((mute.top() - popover.bottom() - 6.0).abs() < f32::EPSILON);

        let slider = video_volume_slider_rect(popover);
        assert!(popover.contains(slider.min));
        assert!(popover.contains(slider.max));
        assert!((popover.bottom() - slider.bottom() - 8.0).abs() < f32::EPSILON);
    }

    #[test]
    fn video_time_readout_uses_larger_fonts_without_crowding_the_label() {
        assert_eq!(video_time_style(360.0), (108.0, 12.0));
        assert_eq!(video_time_style(359.0), (82.0, 10.0));
    }

    #[test]
    fn configurable_select_all_action_reaches_the_canvas() {
        let context = Context::default();
        let mut canvas = CanvasState::default();
        canvas.set_keyboard_input(super::CanvasKeyboardInput {
            select_all: true,
            ..Default::default()
        });
        let mut previews = PreviewManager::new();
        let first = previews.add("first".to_owned(), Pos2::ZERO, Vec2::splat(50.0));
        let second = previews.add(
            "second".to_owned(),
            Pos2::new(100.0, 100.0),
            Vec2::splat(50.0),
        );
        let disabled = previews.add(
            "disabled".to_owned(),
            Pos2::new(200.0, 200.0),
            Vec2::splat(50.0),
        );
        previews.get_mut(disabled).unwrap().left_click_disabled = true;
        let mut captures = CaptureCoordinator::new();

        let _ = context.run(
            RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(500.0))),
                events: vec![Event::PointerMoved(Pos2::new(250.0, 250.0))],
                ..Default::default()
            },
            |context| {
                CentralPanel::default().show(context, |ui| {
                    canvas.ui(ui, &mut previews, &mut captures, context, true);
                });
            },
        );

        assert_eq!(canvas.selection.len(), 2);
        assert!(canvas.selection.contains(&first));
        assert!(canvas.selection.contains(&second));
        assert!(!canvas.selection.contains(&disabled));
    }

    #[test]
    fn left_click_hit_testing_skips_disabled_tiles_but_secondary_targeting_does_not() {
        let canvas = CanvasState::default();
        let mut previews = PreviewManager::new();
        let lower = previews.add("lower".to_owned(), Pos2::ZERO, Vec2::splat(100.0));
        let upper = previews.add("upper".to_owned(), Pos2::ZERO, Vec2::splat(100.0));
        previews.get_mut(upper).unwrap().left_click_disabled = true;
        let canvas_rect = Rect::from_min_size(Pos2::ZERO, Vec2::splat(500.0));
        let pointer = Pos2::new(50.0, 50.0);

        assert_eq!(
            canvas.preview_at_screen(pointer, canvas_rect, &previews),
            Some(upper)
        );
        assert_eq!(
            canvas.left_click_preview_at_screen(pointer, canvas_rect, &previews),
            Some(lower)
        );
    }

    #[test]
    fn deleting_a_selection_keeps_the_whole_batch_for_undo() {
        let context = Context::default();
        let mut canvas = CanvasState::default();
        let mut previews = PreviewManager::new();
        let first = previews.add("first".to_owned(), Pos2::ZERO, Vec2::splat(50.0));
        let second = previews.add(
            "second".to_owned(),
            Pos2::new(100.0, 100.0),
            Vec2::splat(50.0),
        );
        canvas.selection = vec![first, second];
        canvas.set_keyboard_input(super::CanvasKeyboardInput {
            delete_selected: true,
            ..Default::default()
        });
        let mut captures = CaptureCoordinator::new();
        let input = || RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(500.0))),
            ..Default::default()
        };
        let _ = context.run(input(), |context| {
            CentralPanel::default().show(context, |ui| {
                canvas.ui(ui, &mut previews, &mut captures, context, true);
            });
        });
        for id in [first, second] {
            assert!(previews.get(id).unwrap().removing.is_some());
            previews.get_mut(id).unwrap().removing = Some(Instant::now() - Duration::from_secs(1));
        }
        let _ = context.run(input(), |context| {
            CentralPanel::default().show(context, |ui| {
                canvas.ui(ui, &mut previews, &mut captures, context, true);
            });
        });
        assert_eq!(canvas.last_removed.as_ref().unwrap().1.len(), 2);
        assert_eq!(previews.all().count(), 0);
        canvas.last_removed.as_mut().unwrap().0 = Instant::now() - Duration::from_secs(60);
        let _ = context.run(input(), |context| {
            CentralPanel::default().show(context, |ui| {
                canvas.ui(ui, &mut previews, &mut captures, context, true);
            });
        });
        assert!(
            canvas.last_removed.is_some(),
            "toast expiry must not discard undo"
        );
        canvas.request_removed_undo();
        let _ = context.run(input(), |context| {
            CentralPanel::default().show(context, |ui| {
                canvas.ui(ui, &mut previews, &mut captures, context, false);
            });
        });
        assert_eq!(canvas.pending_removed_restore.as_ref().unwrap().len(), 2);
        assert!(canvas.last_removed.is_none());
        // Switching workspaces must discard the old workspace's Undo state.
        canvas.clear_preview_interactions();
        assert!(canvas.last_removed.is_none());
        assert!(canvas.pending_removed_restore.is_none());
    }

    #[test]
    fn undo_during_removal_waits_for_the_current_batch() {
        let context = Context::default();
        let mut canvas = CanvasState::default();
        let mut previews = PreviewManager::new();
        let id = previews.add("tile".to_owned(), Pos2::ZERO, Vec2::splat(50.0));
        previews.start_removal(id);
        previews.get_mut(id).unwrap().removing = Some(Instant::now() + Duration::from_secs(60));
        canvas.request_removed_undo();
        let mut captures = CaptureCoordinator::new();
        let mut render = |canvas: &mut CanvasState, previews: &mut PreviewManager| {
            let _ = context.run(RawInput::default(), |context| {
                CentralPanel::default().show(context, |ui| {
                    canvas.ui(ui, previews, &mut captures, context, true);
                });
            });
        };
        render(&mut canvas, &mut previews);
        assert!(canvas.removed_undo_requested);
        assert!(canvas.pending_removed_restore.is_none());
        previews.get_mut(id).unwrap().removing = Some(Instant::now() - Duration::from_secs(1));
        render(&mut canvas, &mut previews);
        assert!(!canvas.removed_undo_requested);
        assert_eq!(canvas.pending_removed_restore.as_ref().unwrap().len(), 1);
        assert!(canvas.last_removed.is_none());
    }

    #[test]
    fn waking_a_hibernated_window_requires_fresh_identity_matching() {
        let canvas = CanvasState::default();
        let mut previews = PreviewManager::new();
        let id =
            previews.add_for_window(123, 42, "editor".to_owned(), Pos2::ZERO, Vec2::splat(100.0));
        let preview = previews.get_mut(id).unwrap();
        preview.capture_hibernated = true;
        preview.capture_paused = true;
        preview.stream_audio = true;
        let mut captures = CaptureCoordinator::new();
        canvas.update_viewport_culling(
            Rect::from_min_size(Pos2::ZERO, Vec2::splat(500.0)),
            &mut previews,
            &mut captures,
        );
        let preview = previews.get(id).unwrap();
        assert!(preview.is_inactive_window());
        assert!(preview.window_handle.is_none());
        assert!(preview.stream_audio);
        assert!(!captures.is_live(id));
    }

    #[test]
    fn disabled_tile_passes_primary_drag_to_the_tile_underneath() {
        let context = Context::default();
        let mut canvas = CanvasState::default();
        let mut previews = PreviewManager::new();
        let lower = previews.add(
            "lower".to_owned(),
            Pos2::new(100.0, 100.0),
            Vec2::new(200.0, 120.0),
        );
        let upper = previews.add(
            "upper".to_owned(),
            Pos2::new(100.0, 100.0),
            Vec2::new(200.0, 120.0),
        );
        previews.get_mut(upper).unwrap().left_click_disabled = true;
        let mut captures = CaptureCoordinator::new();
        let screen_rect = Rect::from_min_size(Pos2::ZERO, Vec2::splat(500.0));

        let mut run_frame = |events| {
            let _ = context.run(
                RawInput {
                    screen_rect: Some(screen_rect),
                    events,
                    ..Default::default()
                },
                |context| {
                    CentralPanel::default()
                        .frame(egui::Frame::none())
                        .show(context, |ui| {
                            canvas.ui(ui, &mut previews, &mut captures, context, true);
                        });
                },
            );
        };

        let start = Pos2::new(200.0, 160.0);
        let end = Pos2::new(240.0, 160.0);
        run_frame(vec![Event::PointerMoved(start)]);
        run_frame(vec![Event::PointerButton {
            pos: start,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        }]);
        run_frame(vec![Event::PointerButton {
            pos: start,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }]);
        run_frame(vec![Event::PointerButton {
            pos: start,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        }]);
        run_frame(vec![Event::PointerMoved(end)]);
        run_frame(vec![Event::PointerButton {
            pos: end,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }]);

        assert!(previews.get(lower).unwrap().position.x > 100.0);
        assert_eq!(
            previews.get(upper).unwrap().position,
            Pos2::new(100.0, 100.0)
        );
        assert_eq!(canvas.selection, vec![lower]);
    }

    #[test]
    fn disabled_tile_keeps_its_right_click_context_menu() {
        let context = Context::default();
        let mut canvas = CanvasState::default();
        let mut previews = PreviewManager::new();
        let _lower = previews.add(
            "lower".to_owned(),
            Pos2::new(100.0, 100.0),
            Vec2::new(200.0, 120.0),
        );
        let upper = previews.add(
            "upper".to_owned(),
            Pos2::new(100.0, 100.0),
            Vec2::new(200.0, 120.0),
        );
        previews.get_mut(upper).unwrap().left_click_disabled = true;
        let mut captures = CaptureCoordinator::new();
        let screen_rect = Rect::from_min_size(Pos2::ZERO, Vec2::splat(600.0));
        let pointer = Pos2::new(200.0, 160.0);

        let mut run_frame = |events| {
            let _ = context.run(
                RawInput {
                    screen_rect: Some(screen_rect),
                    events,
                    ..Default::default()
                },
                |context| {
                    CentralPanel::default()
                        .frame(egui::Frame::none())
                        .show(context, |ui| {
                            canvas.ui(ui, &mut previews, &mut captures, context, true);
                        });
                },
            );
        };

        run_frame(vec![Event::PointerMoved(pointer)]);
        run_frame(vec![Event::PointerButton {
            pos: pointer,
            button: PointerButton::Secondary,
            pressed: true,
            modifiers: Modifiers::NONE,
        }]);
        run_frame(vec![Event::PointerButton {
            pos: pointer,
            button: PointerButton::Secondary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }]);

        assert_eq!(canvas.selection, vec![upper]);
        assert!(canvas.last_secondary_click.is_none());
        assert_eq!(
            context.layer_id_at(pointer).map(|layer| layer.order),
            Some(Order::Foreground)
        );
    }

    #[test]
    fn foreground_menu_blocks_canvas_scroll_and_secondary_click() {
        let context = Context::default();
        let mut canvas = CanvasState::default();
        let mut previews = PreviewManager::new();
        previews.add(
            "covered tile".to_owned(),
            Pos2::new(100.0, 100.0),
            Vec2::splat(100.0),
        );
        let mut captures = CaptureCoordinator::new();
        let screen_rect = Rect::from_min_size(Pos2::ZERO, Vec2::splat(500.0));
        let menu_pos = Pos2::new(150.0, 150.0);

        let mut render = |events| {
            let _ = context.run(
                RawInput {
                    screen_rect: Some(screen_rect),
                    events,
                    ..Default::default()
                },
                |context| {
                    CentralPanel::default().show(context, |ui| {
                        canvas.ui(ui, &mut previews, &mut captures, context, true);
                    });
                    egui::Area::new(Id::new("test_foreground_menu"))
                        .order(Order::Foreground)
                        .fixed_pos(Pos2::new(100.0, 100.0))
                        .show(context, |ui| {
                            ui.allocate_exact_size(Vec2::splat(100.0), Sense::hover());
                        });
                },
            );
        };

        // Populate egui's layer hit-test data, which is retained between frames.
        render(vec![Event::PointerMoved(menu_pos)]);
        render(vec![Event::PointerMoved(menu_pos)]);
        assert_eq!(
            context.layer_id_at(menu_pos).map(|layer| layer.order),
            Some(Order::Foreground)
        );

        render(vec![Event::PointerButton {
            pos: menu_pos,
            button: PointerButton::Secondary,
            pressed: true,
            modifiers: Modifiers::NONE,
        }]);
        render(vec![Event::PointerButton {
            pos: menu_pos,
            button: PointerButton::Secondary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }]);

        render(vec![
            Event::PointerMoved(menu_pos),
            Event::MouseWheel {
                unit: MouseWheelUnit::Point,
                delta: Vec2::new(0.0, 20.0),
                modifiers: Modifiers::NONE,
            },
        ]);

        render(vec![
            Event::PointerMoved(Pos2::new(350.0, 350.0)),
            Event::MouseWheel {
                unit: MouseWheelUnit::Point,
                delta: Vec2::new(0.0, 20.0),
                modifiers: Modifiers::NONE,
            },
        ]);

        // Only the wheel event outside the foreground area may affect zoom.
        assert!((canvas.zoom - 1.1).abs() < f32::EPSILON);
        assert!(canvas.selection.is_empty());
        assert!(canvas.last_secondary_click.is_none());
    }

    #[test]
    fn stream_audio_badge_sits_left_of_fps_when_hovered() {
        let tile = Rect::from_min_size(Pos2::ZERO, Vec2::new(200.0, 120.0));
        let fps = Rect::from_min_size(
            tile.right_top() + Vec2::new(-72.0, 10.0),
            Vec2::new(36.0, 20.0),
        );
        let sa = stream_audio_badge_rect(tile, true, true);
        assert!(sa.right() <= fps.left() + 0.1);
        assert!(sa.left() > tile.left());
    }

    #[test]
    fn capture_resolution_sits_left_of_fps_when_hovered() {
        let tile = Rect::from_min_size(Pos2::ZERO, Vec2::new(640.0, 360.0));
        let fps = Rect::from_min_size(
            tile.right_top() + Vec2::new(-72.0, 10.0),
            Vec2::new(36.0, 20.0),
        );
        let resolution = capture_resolution_badge_rect(fps);
        assert!(resolution.right() <= fps.left() + 0.1);
        assert!(resolution.left() > tile.left());
    }

    #[test]
    fn capture_resolution_sits_left_of_stream_audio_when_both_are_shown() {
        let tile = Rect::from_min_size(Pos2::ZERO, Vec2::new(640.0, 360.0));
        let sa = stream_audio_badge_rect(tile, true, true);
        let resolution = capture_resolution_badge_rect(sa);
        assert!(resolution.right() <= sa.left() + 0.1);
        assert!(resolution.left() > tile.left());
    }

    #[test]
    fn capture_rect_edges_align_to_physical_pixels() {
        let aligned = pixel_aligned_rect(
            Rect::from_min_max(Pos2::new(0.31, 1.09), Pos2::new(100.42, 50.77)),
            1.25,
        );
        for edge in [
            aligned.left(),
            aligned.top(),
            aligned.right(),
            aligned.bottom(),
        ] {
            assert!((edge * 1.25 - (edge * 1.25).round()).abs() < 0.001);
        }
    }

    #[test]
    fn native_capture_size_maps_each_source_pixel_to_one_screen_pixel() {
        let size = native_capture_canvas_size((1920, 1080), None, 1.25, 2.0);
        assert_eq!(size, Vec2::new(768.0, 432.0));

        let cropped =
            native_capture_canvas_size((1920, 1080), Some((0.25, 0.25, 0.75, 0.75)), 1.25, 2.0);
        assert_eq!(cropped, Vec2::new(384.0, 216.0));
    }

    #[test]
    fn native_capture_action_aligns_and_resizes_the_tile() {
        let mut canvas = CanvasState {
            zoom: 2.0,
            pan: Vec2::new(3.17, -1.43),
            ..Default::default()
        };
        let canvas_rect = Rect::from_min_size(Pos2::new(10.3, 20.7), Vec2::splat(1200.0));
        let mut previews = PreviewManager::new();
        let id = previews.add_for_window(
            1,
            42,
            "window".to_owned(),
            Pos2::new(7.13, 9.81),
            Vec2::splat(100.0),
        );

        canvas.set_native_capture_size(id, (1920, 1080), canvas_rect, 1.25, &mut previews);

        let preview = previews.get(id).unwrap();
        assert_eq!(preview.size, Vec2::new(768.0, 432.0));
        let screen = canvas.canvas_rect_to_screen(preview.rect(), canvas_rect);
        assert_eq!((screen.width() * 1.25).round() as u32, 1920);
        assert_eq!((screen.height() * 1.25).round() as u32, 1080);
        assert!((screen.left() * 1.25 - (screen.left() * 1.25).round()).abs() < 0.001);
        assert!((screen.top() * 1.25 - (screen.top() * 1.25).round()).abs() < 0.001);
    }

    #[test]
    fn native_capture_action_resizes_a_pinned_tile_in_screen_space() {
        let mut canvas = CanvasState {
            zoom: 0.25,
            pan: Vec2::new(900.0, -400.0),
            ..Default::default()
        };
        let canvas_rect = Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::splat(1200.0));
        let mut previews = PreviewManager::new();
        let id = previews.add_for_window(
            1,
            42,
            "window".to_owned(),
            Pos2::new(7.0, 9.0),
            Vec2::splat(100.0),
        );
        previews.get_mut(id).unwrap().viewport_pin = Some(crate::preview::ViewportPin::from_rect(
            Rect::from_min_size(Pos2::new(100.0, 80.0), Vec2::splat(100.0)),
            canvas_rect,
        ));

        canvas.set_native_capture_size(id, (1920, 1080), canvas_rect, 1.25, &mut previews);

        let preview = previews.get(id).unwrap();
        let screen = preview.viewport_pin.unwrap().rect(canvas_rect);
        assert_eq!((screen.width() * 1.25).round() as u32, 1920);
        assert_eq!((screen.height() * 1.25).round() as u32, 1080);
        assert!(preview.viewport_pin.is_some());
    }

    #[test]
    fn stream_audio_on_badge_is_wider_than_off() {
        let tile = Rect::from_min_size(Pos2::ZERO, Vec2::splat(200.0));
        let off = stream_audio_badge_rect(tile, false, true);
        let on = stream_audio_badge_rect(tile, true, true);
        assert!(on.width() > off.width());
    }

    #[test]
    fn stream_audio_toggle_flips_window_tiles_only() {
        let mut canvas = CanvasState::default();
        let mut previews = PreviewManager::new();
        let window =
            previews.add_for_window(1, 42, "game".to_owned(), Pos2::ZERO, Vec2::splat(100.0));
        let other = previews.add("image".to_owned(), Pos2::ZERO, Vec2::splat(100.0));

        canvas.pending_stream_audio_toggles.extend([window, other]);
        canvas.apply_pending_stream_audio_toggles(&mut previews);

        assert!(previews.get(window).unwrap().stream_audio);
        assert!(!previews.get(other).unwrap().stream_audio);
    }

    #[test]
    fn enabled_stream_audio_badge_is_visible_only_while_hovering_the_tile() {
        fn render_with_pointer(pointer: Pos2) -> eframe::egui::FullOutput {
            let context = Context::default();
            let mut canvas = CanvasState::default();
            let mut previews = PreviewManager::new();
            let id = previews.add_for_window(
                1,
                42,
                "window".to_owned(),
                Pos2::new(100.0, 100.0),
                Vec2::new(240.0, 160.0),
            );
            previews.get_mut(id).unwrap().stream_audio = true;
            let mut captures = CaptureCoordinator::new();

            context.run(
                RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(600.0))),
                    events: vec![Event::PointerMoved(pointer)],
                    ..Default::default()
                },
                |context| {
                    CentralPanel::default()
                        .frame(egui::Frame::none())
                        .show(context, |ui| {
                            canvas.ui(ui, &mut previews, &mut captures, context, true);
                        });
                },
            )
        }

        let hovered = render_with_pointer(Pos2::new(150.0, 150.0));
        assert!(hovered.shapes.iter().any(|shape| {
            matches!(
                &shape.shape,
                Shape::Text(text) if text.galley.text() == "Stream Audio: On"
            )
        }));

        let not_hovered = render_with_pointer(Pos2::new(500.0, 500.0));
        assert!(!not_hovered.shapes.iter().any(|shape| {
            matches!(
                &shape.shape,
                Shape::Text(text) if text.galley.text().starts_with("Stream Audio")
            )
        }));
    }

    #[test]
    fn pinned_tile_stays_fixed_through_canvas_pan_and_zoom() {
        let mut canvas = CanvasState {
            pan: Vec2::new(20.0, -10.0),
            zoom: 1.5,
            ..Default::default()
        };
        let viewport = Rect::from_min_size(Pos2::new(40.0, 30.0), Vec2::new(1200.0, 700.0));
        let mut previews = PreviewManager::new();
        let id = previews.add_for_spout(
            "VTube Studio".to_owned(),
            Pos2::new(100.0, 80.0),
            Vec2::new(320.0, 480.0),
            FpsPreset::Medium,
        );
        let before = canvas.preview_screen_rect(previews.get(id).unwrap(), viewport);

        canvas.toggle_viewport_pin(id, viewport, &mut previews);
        canvas.pan = Vec2::new(-500.0, 900.0);
        canvas.zoom = 0.25;

        assert_eq!(
            canvas.preview_screen_rect(previews.get(id).unwrap(), viewport),
            before
        );
        assert_eq!(
            live_capture_display_size(previews.get(id).unwrap(), 0.25),
            before.size()
        );
    }

    #[test]
    fn left_drag_repositions_a_pinned_tile() {
        let context = Context::default();
        let mut canvas = CanvasState::default();
        let mut previews = PreviewManager::new();
        let id = previews.add_for_spout(
            "VTube Studio".to_owned(),
            Pos2::new(100.0, 80.0),
            Vec2::new(200.0, 300.0),
            FpsPreset::Medium,
        );
        let screen_rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));
        previews.get_mut(id).unwrap().viewport_pin = Some(crate::preview::ViewportPin::from_rect(
            Rect::from_min_size(Pos2::new(100.0, 80.0), Vec2::new(200.0, 300.0)),
            screen_rect,
        ));
        let mut captures = CaptureCoordinator::new();
        let mut run_frame = |events| {
            let _ = context.run(
                RawInput {
                    screen_rect: Some(screen_rect),
                    events,
                    ..Default::default()
                },
                |context| {
                    CentralPanel::default()
                        .frame(egui::Frame::none())
                        .show(context, |ui| {
                            canvas.ui(ui, &mut previews, &mut captures, context, true);
                        });
                },
            );
        };

        let start = Pos2::new(200.0, 200.0);
        let end = Pos2::new(280.0, 240.0);
        run_frame(vec![Event::PointerMoved(start)]);
        run_frame(vec![Event::PointerButton {
            pos: start,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        }]);
        run_frame(vec![Event::PointerMoved(end)]);
        run_frame(vec![Event::PointerButton {
            pos: end,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }]);

        let moved = previews
            .get(id)
            .unwrap()
            .viewport_pin
            .unwrap()
            .rect(canvas.last_screen_rect.unwrap());
        assert_eq!(moved.min, Pos2::new(180.0, 120.0));
        assert!(previews.get(id).unwrap().viewport_pin.is_some());
    }

    #[test]
    fn unpinning_preserves_the_current_screen_rect() {
        let mut canvas = CanvasState {
            pan: Vec2::new(-60.0, 25.0),
            zoom: 0.75,
            ..Default::default()
        };
        let viewport = Rect::from_min_size(Pos2::new(25.0, 15.0), Vec2::new(1000.0, 700.0));
        let mut previews = PreviewManager::new();
        let id = previews.add_for_spout(
            "avatar".to_owned(),
            Pos2::new(220.0, 160.0),
            Vec2::new(240.0, 360.0),
            FpsPreset::Medium,
        );
        canvas.toggle_viewport_pin(id, viewport, &mut previews);
        let pinned_rect = canvas.preview_screen_rect(previews.get(id).unwrap(), viewport);

        canvas.toggle_viewport_pin(id, viewport, &mut previews);

        assert!(previews.get(id).unwrap().viewport_pin.is_none());
        assert_eq!(
            canvas.preview_screen_rect(previews.get(id).unwrap(), viewport),
            pinned_rect
        );
    }

    #[test]
    fn viewport_pin_toggle_supports_non_spout_tiles() {
        let mut canvas = CanvasState::default();
        let viewport = Rect::from_min_size(Pos2::ZERO, Vec2::splat(500.0));
        let mut previews = PreviewManager::new();
        let id = previews.add("image".to_owned(), Pos2::ZERO, Vec2::splat(100.0));

        let before = canvas.preview_screen_rect(previews.get(id).unwrap(), viewport);
        canvas.toggle_viewport_pin(id, viewport, &mut previews);

        assert!(previews.get(id).unwrap().viewport_pin.is_some());
        canvas.pan = Vec2::new(400.0, -200.0);
        canvas.zoom = 2.0;
        assert_eq!(
            canvas.preview_screen_rect(previews.get(id).unwrap(), viewport),
            before
        );
    }

    #[test]
    fn disabling_left_click_clears_selection_and_requests_browser_interaction_exit() {
        let mut canvas = CanvasState::default();
        let mut previews = PreviewManager::new();
        let id = previews.add_browser_placeholder(
            "https://example.com".to_owned(),
            Pos2::ZERO,
            Vec2::new(320.0, 180.0),
            FpsPreset::Medium,
        );
        canvas.selection = vec![id];
        canvas.interactive_browser = Some(id);

        canvas.toggle_left_click_disabled(id, &mut previews);

        assert!(previews.get(id).unwrap().left_click_disabled);
        assert!(canvas.selection.is_empty());
        assert_eq!(canvas.last_double_clicked, Some(id));

        canvas.toggle_left_click_disabled(id, &mut previews);
        assert!(!previews.get(id).unwrap().left_click_disabled);
    }

    #[test]
    fn browser_aspect_lock_can_preserve_a_portrait_shape() {
        let mut canvas = CanvasState::default();
        let mut previews = PreviewManager::new();
        let id = previews.add_browser_placeholder(
            "https://example.com".to_owned(),
            Pos2::ZERO,
            Vec2::new(320.0, 180.0),
            FpsPreset::Medium,
        );

        canvas.toggle_aspect_ratio_lock(id, &mut previews);
        assert!(!previews.get(id).unwrap().lock_aspect_ratio);

        previews.get_mut(id).unwrap().size = Vec2::new(180.0, 320.0);
        canvas.toggle_aspect_ratio_lock(id, &mut previews);
        let preview = previews.get(id).unwrap();
        assert!(preview.lock_aspect_ratio);
        assert!((preview.source_aspect_ratio - 180.0 / 320.0).abs() < f32::EPSILON);
    }

    #[test]
    fn video_time_format_handles_hours_and_missing_values() {
        assert_eq!(format_time(None), "0:00");
        assert_eq!(format_time(Some(65.9)), "1:05");
        assert_eq!(format_time(Some(3661.0)), "1:01:01");
        assert_eq!(format_time(Some(f64::NAN)), "0:00");
    }

    #[test]
    fn video_placeholder_animation_matches_status() {
        assert!(video_placeholder_content(&VideoTileStatus::Starting).2);
        assert!(video_placeholder_content(&VideoTileStatus::Buffering).2);
        assert!(!video_placeholder_content(&VideoTileStatus::PausedOnRestore).2);
        let failed = VideoTileStatus::Failed("raw stderr that should stay hidden".to_owned());
        let content = video_placeholder_content(&failed);
        assert_eq!(content.0, "Video unavailable");
        assert!(!content.2);
        assert!(!content.1.contains("stderr"));
    }

    #[test]
    fn window_capture_placeholder_stops_animating_when_failed() {
        assert_eq!(window_capture_placeholder_content(true).0, "Capture failed");
        assert!(!window_capture_placeholder_content(true).2);
        assert_eq!(window_capture_placeholder_content(false).0, "Connecting...");
        assert!(window_capture_placeholder_content(false).2);
    }

    #[test]
    fn unavailable_image_placeholder_offers_recovery() {
        let (title, detail) = media_placeholder_content();
        assert_eq!(title, "Image unavailable");
        assert!(detail.contains("retry"));
        assert!(detail.contains("locate"));
    }

    #[test]
    fn fast_tile_drag_stops_exactly_at_release_position() {
        fn run_frame(
            context: &Context,
            canvas: &mut CanvasState,
            previews: &mut PreviewManager,
            captures: &mut CaptureCoordinator,
            screen_rect: Rect,
            time: f64,
            events: Vec<Event>,
        ) {
            let _ = context.run(
                RawInput {
                    screen_rect: Some(screen_rect),
                    time: Some(time),
                    events,
                    ..Default::default()
                },
                |context| {
                    CentralPanel::default()
                        .frame(egui::Frame::none())
                        .show(context, |ui| {
                            canvas.ui(ui, previews, captures, context, true);
                        });
                },
            );
        }

        let context = Context::default();
        let mut canvas = CanvasState::default();
        let mut previews = PreviewManager::new();
        let id = previews.add(
            "tile".to_owned(),
            Pos2::new(100.0, 100.0),
            Vec2::new(200.0, 120.0),
        );
        let mut captures = CaptureCoordinator::new();
        let screen_rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));
        let start = Pos2::new(180.0, 150.0);
        let midway = Pos2::new(260.0, 150.0);
        let end = Pos2::new(380.0, 150.0);

        run_frame(
            &context,
            &mut canvas,
            &mut previews,
            &mut captures,
            screen_rect,
            0.00,
            vec![Event::PointerMoved(start)],
        );
        run_frame(
            &context,
            &mut canvas,
            &mut previews,
            &mut captures,
            screen_rect,
            0.01,
            vec![Event::PointerButton {
                pos: start,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            }],
        );
        run_frame(
            &context,
            &mut canvas,
            &mut previews,
            &mut captures,
            screen_rect,
            0.02,
            vec![Event::PointerMoved(midway)],
        );
        run_frame(
            &context,
            &mut canvas,
            &mut previews,
            &mut captures,
            screen_rect,
            0.03,
            vec![Event::PointerMoved(end)],
        );
        run_frame(
            &context,
            &mut canvas,
            &mut previews,
            &mut captures,
            screen_rect,
            0.04,
            vec![Event::PointerButton {
                pos: end,
                button: PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            }],
        );

        let released_position = previews.get(id).unwrap().position;
        run_frame(
            &context,
            &mut canvas,
            &mut previews,
            &mut captures,
            screen_rect,
            0.10,
            Vec::new(),
        );
        run_frame(
            &context,
            &mut canvas,
            &mut previews,
            &mut captures,
            screen_rect,
            0.20,
            Vec::new(),
        );

        assert_ne!(released_position, Pos2::new(100.0, 100.0));
        assert_eq!(previews.get(id).unwrap().position, released_position);
    }

    #[test]
    fn canvas_only_hides_empty_canvas_overlays() {
        let context = Context::default();
        let mut canvas = CanvasState::default();
        let mut previews = PreviewManager::new();
        let mut captures = CaptureCoordinator::new();
        let output = context.run(
            RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(500.0))),
                ..Default::default()
            },
            |context| {
                CentralPanel::default().show(context, |ui| {
                    canvas.ui(ui, &mut previews, &mut captures, context, false);
                });
            },
        );

        assert!(!output
            .shapes
            .iter()
            .any(|shape| { matches!(shape.shape, Shape::Text(_) | Shape::LineSegment { .. }) }));
    }

    #[test]
    fn canvas_only_cancels_active_handle_drag() {
        let context = Context::default();
        let mut canvas = CanvasState::default();
        canvas.drag_state = Some(DragState::Resizing {
            id: PreviewId(1),
            handle: ResizeHandle::BottomRight,
            start_rect: Rect::from_min_size(Pos2::ZERO, Vec2::splat(100.0)),
            start_mouse: Pos2::new(100.0, 100.0),
            aspect_ratio: Some(1.0),
            screen_space: false,
        });
        let mut previews = PreviewManager::new();
        let mut captures = CaptureCoordinator::new();

        let _ = context.run(RawInput::default(), |context| {
            CentralPanel::default().show(context, |ui| {
                canvas.ui(ui, &mut previews, &mut captures, context, false);
            });
        });

        assert!(canvas.drag_state.is_none());
    }

    #[test]
    fn unlocked_edge_resize_changes_only_the_grabbed_axis() {
        let start = Rect::from_min_size(Pos2::new(40.0, 80.0), Vec2::new(200.0, 360.0));

        let taller = apply_resize(ResizeHandle::Bottom, start, Vec2::new(90.0, 80.0), None);
        assert!((taller.width() - 200.0).abs() < f32::EPSILON);
        assert!((taller.height() - 440.0).abs() < f32::EPSILON);
        assert!((taller.min.x - 40.0).abs() < f32::EPSILON);

        let wider = apply_resize(ResizeHandle::Right, start, Vec2::new(120.0, 90.0), None);
        assert!((wider.width() - 320.0).abs() < f32::EPSILON);
        assert!((wider.height() - 360.0).abs() < f32::EPSILON);
        assert!((wider.min.y - 80.0).abs() < f32::EPSILON);
    }

    #[test]
    fn locked_edge_resize_still_keeps_aspect_ratio() {
        let start = Rect::from_min_size(Pos2::new(40.0, 80.0), Vec2::new(200.0, 100.0));
        let resized = apply_resize(ResizeHandle::Right, start, Vec2::new(100.0, 0.0), Some(2.0));
        assert!((resized.width() - 300.0).abs() < f32::EPSILON);
        assert!((resized.height() - 150.0).abs() < f32::EPSILON);
    }

    #[test]
    fn crop_moves_the_grabbed_edge_without_stretching_or_resizing_capture_backing() {
        let start = Rect::from_min_size(Pos2::new(100.0, 80.0), Vec2::new(200.0, 100.0));
        let (crop, rect) = apply_crop(
            ResizeHandle::Right,
            (0.0, 0.0, 1.0, 1.0),
            start,
            Vec2::new(-50.0, 25.0),
        );

        assert_eq!(crop, (0.0, 0.0, 0.75, 1.0));
        assert_eq!(rect.min, start.min);
        assert_eq!(rect.size(), Vec2::new(150.0, 100.0));
        assert_eq!(
            window_capture_target(start.width(), start.height(), 1.0, None),
            window_capture_target(rect.width(), rect.height(), 1.0, Some(crop))
        );
        assert_eq!(
            rect_for_crop_change(crop, (0.0, 0.0, 1.0, 1.0), rect),
            start
        );
    }

    #[test]
    fn repeated_crop_uses_the_visible_crop_span_as_its_drag_scale() {
        let start = Rect::from_min_size(Pos2::new(50.0, 40.0), Vec2::new(100.0, 50.0));
        let (crop, rect) = apply_crop(
            ResizeHandle::Left,
            (0.25, 0.25, 0.75, 0.75),
            start,
            Vec2::new(25.0, 0.0),
        );

        assert_eq!(crop, (0.375, 0.25, 0.75, 0.75));
        assert_eq!(rect.min, Pos2::new(75.0, 40.0));
        assert_eq!(rect.size(), Vec2::new(75.0, 50.0));
    }

    fn alt_crop_right_edge(
        mut previews: PreviewManager,
        id: PreviewId,
    ) -> (CanvasState, PreviewManager) {
        let context = Context::default();
        let mut canvas = CanvasState {
            pan: Vec2::new(17.0, -9.0),
            ..Default::default()
        };
        canvas.selection = vec![id];
        let mut captures = CaptureCoordinator::new();
        let screen_rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(700.0, 500.0));
        let alt = Modifiers {
            alt: true,
            ..Default::default()
        };
        let start = canvas
            .canvas_rect_to_screen(previews.get(id).unwrap().rect(), screen_rect)
            .right_center();
        let end = start - Vec2::new(50.0, 0.0);
        let mut run_frame = |events, modifiers| {
            let _ = context.run(
                RawInput {
                    screen_rect: Some(screen_rect),
                    events,
                    modifiers,
                    ..Default::default()
                },
                |context| {
                    CentralPanel::default()
                        .frame(egui::Frame::none())
                        .show(context, |ui| {
                            canvas.ui(ui, &mut previews, &mut captures, context, true);
                        });
                },
            );
        };

        run_frame(vec![Event::PointerMoved(start)], alt);
        run_frame(
            vec![Event::PointerButton {
                pos: start,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: alt,
            }],
            alt,
        );
        run_frame(vec![Event::PointerMoved(end)], alt);
        run_frame(
            vec![Event::PointerButton {
                pos: end,
                button: PointerButton::Primary,
                pressed: false,
                modifiers: alt,
            }],
            alt,
        );

        (canvas, previews)
    }

    #[test]
    fn alt_crop_drag_does_not_pan_the_canvas() {
        let mut previews = PreviewManager::new();
        let id = previews.add_for_window(
            1,
            42,
            "window".to_owned(),
            Pos2::new(100.0, 100.0),
            Vec2::new(200.0, 100.0),
        );
        previews
            .get_mut(id)
            .unwrap()
            .update_frame(200, 100, vec![0; 200 * 100 * 4]);
        let (canvas, previews) = alt_crop_right_edge(previews, id);

        let preview = previews.get(id).unwrap();
        assert_eq!(canvas.pan, Vec2::new(17.0, -9.0));
        assert_eq!(preview.crop_uv, Some((0.0, 0.0, 0.75, 1.0)));
        assert_eq!(preview.size, Vec2::new(150.0, 100.0));
        assert!((preview.source_aspect_ratio - 1.5).abs() < 0.001);
    }

    #[test]
    fn alt_crop_drag_crops_an_image_tile() {
        let mut previews = PreviewManager::new();
        let id = previews.add_media(
            PathBuf::from("image.png"),
            "image".to_owned(),
            vec![crate::media::MediaFrame {
                width: 200,
                height: 100,
                rgba: vec![0; 200 * 100 * 4],
                duration: Duration::from_millis(100),
            }],
            Pos2::new(100.0, 100.0),
            Vec2::new(200.0, 100.0),
        );
        let (mut canvas, mut previews) = alt_crop_right_edge(previews, id);

        let preview = previews.get(id).unwrap();
        assert_eq!(preview.crop_uv, Some((0.0, 0.0, 0.75, 1.0)));
        assert_eq!(preview.size, Vec2::new(150.0, 100.0));
        assert!((preview.source_aspect_ratio - 1.5).abs() < 0.001);

        canvas.set_preview_crop(
            id,
            None,
            Rect::from_min_size(Pos2::ZERO, Vec2::new(700.0, 500.0)),
            &mut previews,
        );
        let preview = previews.get(id).unwrap();
        assert_eq!(preview.crop_uv, None);
        assert_eq!(preview.size, Vec2::new(200.0, 100.0));
        assert!((preview.source_aspect_ratio - 2.0).abs() < 0.001);
    }

    #[test]
    fn alt_crop_drag_crops_a_browser_tile() {
        let mut previews = PreviewManager::new();
        let id = previews.add_browser_placeholder(
            "https://example.com".to_owned(),
            Pos2::new(100.0, 100.0),
            Vec2::new(200.0, 100.0),
            FpsPreset::Medium,
        );
        previews
            .get_mut(id)
            .unwrap()
            .update_frame(200, 100, vec![0; 200 * 100 * 4]);
        let (_, previews) = alt_crop_right_edge(previews, id);

        let preview = previews.get(id).unwrap();
        assert_eq!(preview.crop_uv, Some((0.0, 0.0, 0.75, 1.0)));
        assert_eq!(preview.size, Vec2::new(150.0, 100.0));
        assert!((preview.source_aspect_ratio - 1.5).abs() < 0.001);
    }

    #[test]
    fn alt_crop_drag_crops_a_direct_video_without_a_capture_frame() {
        let mut previews = PreviewManager::new();
        let id = previews.add_video_placeholder(
            VideoSource::LocalFile {
                path: PathBuf::from("video.mp4"),
            },
            "video".to_owned(),
            Pos2::new(100.0, 100.0),
            Vec2::new(200.0, 100.0),
            FpsPreset::Medium,
            false,
        );
        assert!(previews.get(id).unwrap().frame_size.is_none());
        let (mut canvas, mut previews) = alt_crop_right_edge(previews, id);

        let preview = previews.get(id).unwrap();
        assert_eq!(preview.crop_uv, Some((0.0, 0.0, 0.75, 1.0)));
        assert_eq!(preview.size, Vec2::new(150.0, 100.0));
        assert!((preview.source_aspect_ratio - 1.5).abs() < 0.001);

        canvas.set_preview_crop(
            id,
            None,
            Rect::from_min_size(Pos2::ZERO, Vec2::new(700.0, 500.0)),
            &mut previews,
        );
        let preview = previews.get(id).unwrap();
        assert_eq!(preview.crop_uv, None);
        assert_eq!(preview.size, Vec2::new(200.0, 100.0));
        assert!((preview.source_aspect_ratio - 2.0).abs() < 0.001);
    }

    #[test]
    fn canvas_only_does_not_show_resize_cursor_for_hidden_handles() {
        let context = Context::default();
        let mut canvas = CanvasState::default();
        let mut previews = PreviewManager::new();
        let id = previews.add(
            "tile".to_owned(),
            Pos2::new(100.0, 100.0),
            Vec2::splat(100.0),
        );
        canvas.selection = vec![id];
        let mut captures = CaptureCoordinator::new();
        let output = context.run(
            RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(500.0))),
                events: vec![Event::PointerMoved(Pos2::new(200.0, 200.0))],
                ..Default::default()
            },
            |context| {
                CentralPanel::default()
                    .frame(egui::Frame::none())
                    .show(context, |ui| {
                        canvas.ui(ui, &mut previews, &mut captures, context, false);
                    });
            },
        );

        assert_eq!(output.platform_output.cursor_icon, CursorIcon::Default);
    }

    #[test]
    fn tile_focus_hides_zoom_badge_and_resize_cursor() {
        let context = Context::default();
        let mut canvas = CanvasState::default();
        let mut previews = PreviewManager::new();
        let id = previews.add(
            "tile".to_owned(),
            Pos2::new(100.0, 100.0),
            Vec2::new(400.0, 225.0),
        );
        let tile = previews.get(id).unwrap().rect();
        let screen_rect = Rect::from_min_size(Pos2::ZERO, Vec2::splat(500.0));
        canvas.focus_on_tile(id, tile, screen_rect);
        let mut captures = CaptureCoordinator::new();
        let focused = canvas.canvas_rect_to_screen(tile, screen_rect);
        let output = context.run(
            RawInput {
                screen_rect: Some(screen_rect),
                events: vec![Event::PointerMoved(focused.right_bottom())],
                ..Default::default()
            },
            |context| {
                CentralPanel::default()
                    .frame(egui::Frame::none())
                    .show(context, |ui| {
                        canvas.ui(ui, &mut previews, &mut captures, context, true);
                    });
            },
        );

        assert!(canvas.is_focusing_tile());
        assert_eq!(output.platform_output.cursor_icon, CursorIcon::Default);
        assert!(!output.shapes.iter().any(|shape| {
            matches!(
                &shape.shape,
                Shape::Text(text) if text.galley.text().contains('%')
            )
        }));
    }

    #[test]
    fn left_drag_on_empty_canvas_marquee_selects_intersecting_tiles() {
        let context = Context::default();
        let mut canvas = CanvasState::default();
        let mut previews = PreviewManager::new();
        let first = previews.add(
            "first".to_owned(),
            Pos2::new(100.0, 100.0),
            Vec2::splat(100.0),
        );
        let second = previews.add(
            "second".to_owned(),
            Pos2::new(260.0, 100.0),
            Vec2::splat(100.0),
        );
        let outside = previews.add(
            "outside".to_owned(),
            Pos2::new(440.0, 100.0),
            Vec2::splat(100.0),
        );
        canvas.selection = vec![outside];
        let mut captures = CaptureCoordinator::new();
        let screen_rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(600.0, 400.0));
        let mut run_frame = |events| {
            let _ = context.run(
                RawInput {
                    screen_rect: Some(screen_rect),
                    events,
                    ..Default::default()
                },
                |context| {
                    CentralPanel::default()
                        .frame(egui::Frame::none())
                        .show(context, |ui| {
                            canvas.ui(ui, &mut previews, &mut captures, context, true);
                        });
                },
            );
        };

        let start = Pos2::new(50.0, 50.0);
        let end = Pos2::new(390.0, 240.0);
        run_frame(vec![Event::PointerMoved(start)]);
        run_frame(vec![Event::PointerButton {
            pos: start,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        }]);
        run_frame(vec![Event::PointerMoved(end)]);
        run_frame(vec![Event::PointerButton {
            pos: end,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }]);

        assert!(canvas.selection.contains(&first));
        assert!(canvas.selection.contains(&second));
        assert!(!canvas.selection.contains(&outside));
    }

    #[test]
    fn resize_handle_outside_tile_wins_over_empty_canvas_marquee() {
        let context = Context::default();
        let mut canvas = CanvasState::default();
        let mut previews = PreviewManager::new();
        let id = previews.add(
            "resizable".to_owned(),
            Pos2::new(100.0, 100.0),
            Vec2::splat(100.0),
        );
        canvas.selection = vec![id];
        let mut captures = CaptureCoordinator::new();
        let screen_rect = Rect::from_min_size(Pos2::ZERO, Vec2::splat(500.0));
        let mut run_frame = |events| {
            let _ = context.run(
                RawInput {
                    screen_rect: Some(screen_rect),
                    events,
                    ..Default::default()
                },
                |context| {
                    CentralPanel::default()
                        .frame(egui::Frame::none())
                        .show(context, |ui| {
                            canvas.ui(ui, &mut previews, &mut captures, context, true);
                        });
                },
            );
        };

        // The right handle is centered at (200, 150). This press is outside
        // the tile itself but inside the handle's deliberately larger hit box.
        let start = Pos2::new(206.5, 150.0);
        let end = Pos2::new(246.5, 150.0);
        run_frame(vec![Event::PointerMoved(start)]);
        run_frame(vec![Event::PointerButton {
            pos: start,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        }]);
        run_frame(vec![Event::PointerMoved(end)]);
        run_frame(vec![Event::PointerButton {
            pos: end,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }]);

        assert!(canvas.marquee.is_none());
        assert!(previews.get(id).unwrap().size.x > 100.0);
    }

    #[test]
    fn group_activity_action_is_queued_for_every_selected_tile() {
        let mut canvas = CanvasState::default();
        let ids = [PreviewId(2), PreviewId(7), PreviewId(9)];

        canvas.queue_activity_for_selection(&ids, TileActivityAction::Freeze);

        assert_eq!(
            canvas.pending_tile_activity_actions,
            vec![
                (PreviewId(2), TileActivityAction::Freeze),
                (PreviewId(7), TileActivityAction::Freeze),
                (PreviewId(9), TileActivityAction::Freeze),
            ]
        );
    }

    #[test]
    fn viewport_culling_never_resumes_a_manually_frozen_tile() {
        let canvas = CanvasState::default();
        let mut previews = PreviewManager::new();
        let id = previews.add(
            "frozen".to_owned(),
            Pos2::new(100.0, 100.0),
            Vec2::splat(100.0),
        );
        let preview = previews.get_mut(id).unwrap();
        preview.manually_frozen = true;
        preview.capture_paused = true;
        let mut captures = CaptureCoordinator::new();

        canvas.update_viewport_culling(
            Rect::from_min_size(Pos2::ZERO, Vec2::splat(500.0)),
            &mut previews,
            &mut captures,
        );

        assert!(previews.get(id).unwrap().capture_paused);
    }

    #[test]
    fn distant_window_capture_hibernates_without_changing_audio_intent() {
        let canvas = CanvasState::default();
        let mut previews = PreviewManager::new();
        let id = previews.add_for_window(
            1,
            42,
            "podcast".to_owned(),
            Pos2::new(2_000.0, 2_000.0),
            Vec2::splat(100.0),
        );
        let preview = previews.get_mut(id).unwrap();
        preview.stream_audio = true;
        preview.capture_paused = true;
        preview.capture_offscreen_since =
            Some(Instant::now() - super::CAPTURE_HIBERNATE_AFTER - Duration::from_secs(1));
        let mut captures = CaptureCoordinator::new();

        canvas.update_viewport_culling(
            Rect::from_min_size(Pos2::ZERO, Vec2::splat(500.0)),
            &mut previews,
            &mut captures,
        );

        let preview = previews.get(id).unwrap();
        assert!(preview.capture_hibernated);
        assert!(preview.capture_paused);
        assert!(preview.stream_audio);
    }

    #[test]
    fn nearby_offscreen_capture_pauses_without_starting_hibernation_timer() {
        let canvas = CanvasState::default();
        let mut previews = PreviewManager::new();
        let id = previews.add_for_window(
            1,
            42,
            "near edge".to_owned(),
            Pos2::new(510.0, 100.0),
            Vec2::splat(100.0),
        );
        let mut captures = CaptureCoordinator::new();

        canvas.update_viewport_culling(
            Rect::from_min_size(Pos2::ZERO, Vec2::splat(500.0)),
            &mut previews,
            &mut captures,
        );

        let preview = previews.get(id).unwrap();
        assert!(preview.capture_paused);
        assert!(!preview.capture_hibernated);
        assert!(preview.capture_offscreen_since.is_none());
    }

    #[test]
    fn tile_close_button_starts_removal() {
        let context = Context::default();
        let mut canvas = CanvasState::default();
        let mut previews = PreviewManager::new();
        let id = previews.add(
            "closable".to_owned(),
            Pos2::new(100.0, 100.0),
            Vec2::new(200.0, 120.0),
        );
        let mut captures = CaptureCoordinator::new();
        let screen_rect = Rect::from_min_size(Pos2::ZERO, Vec2::splat(500.0));

        let mut run_frame = |events| {
            let _ = context.run(
                RawInput {
                    screen_rect: Some(screen_rect),
                    events,
                    ..Default::default()
                },
                |context| {
                    CentralPanel::default()
                        .frame(egui::Frame::none())
                        .show(context, |ui| {
                            canvas.ui(ui, &mut previews, &mut captures, context, true);
                        });
                },
            );
        };

        let close_button_center = Pos2::new(280.0, 120.0);
        run_frame(vec![Event::PointerMoved(close_button_center)]);
        run_frame(vec![Event::PointerButton {
            pos: close_button_center,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        }]);
        run_frame(vec![Event::PointerButton {
            pos: close_button_center,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }]);

        assert!(previews.get(id).unwrap().removing.is_some());
    }

    #[test]
    fn paused_video_play_button_queues_explicit_resume() {
        let context = Context::default();
        let mut canvas = CanvasState::default();
        let mut previews = PreviewManager::new();
        let id = previews.add_video_placeholder(
            VideoSource::LocalFile {
                path: PathBuf::from("play-button-test.mp4"),
            },
            "Paused video".to_owned(),
            Pos2::new(100.0, 100.0),
            Vec2::new(400.0, 225.0),
            FpsPreset::Medium,
            false,
        );
        let preview = previews.get_mut(id).unwrap();
        preview.created_at = Instant::now() - Duration::from_secs(1);
        preview.video_status = VideoTileStatus::Ready;
        preview.video_playback.connected = true;
        preview.video_playback.paused = true;

        let mut captures = CaptureCoordinator::new();
        let screen_rect = Rect::from_min_size(Pos2::ZERO, Vec2::splat(600.0));
        let mut run_frame = |events| {
            let _ = context.run(
                RawInput {
                    screen_rect: Some(screen_rect),
                    events,
                    ..Default::default()
                },
                |context| {
                    CentralPanel::default()
                        .frame(egui::Frame::none())
                        .show(context, |ui| {
                            canvas.ui(ui, &mut previews, &mut captures, context, true);
                        });
                },
            );
        };

        // Tile bottom is y=325; the play control occupies x=108..134, y=290..316.
        let play_button_center = Pos2::new(121.0, 303.0);
        run_frame(vec![Event::PointerMoved(play_button_center)]);
        run_frame(vec![Event::PointerButton {
            pos: play_button_center,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        }]);
        run_frame(vec![Event::PointerButton {
            pos: play_button_center,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }]);

        assert!(canvas
            .pending_video_actions
            .contains(&(id, VideoAction::SetPaused(false))));
    }

    #[test]
    fn playlist_row_queues_file_selection() {
        let root =
            std::env::temp_dir().join(format!("pluriview-canvas-playlist-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let video = root.join("01 opening.mp4");
        std::fs::write(&video, b"").unwrap();

        let context = Context::default();
        let mut canvas = CanvasState::default();
        let mut previews = PreviewManager::new();
        let playlist = FolderPlaylist::scan(root.clone(), None).unwrap();
        let id = previews.add_folder_playlist(
            playlist,
            "Test folder".to_owned(),
            Pos2::new(100.0, 100.0),
            Vec2::new(340.0, 360.0),
            1,
            None,
        );
        previews.get_mut(id).unwrap().created_at = Instant::now() - Duration::from_secs(1);
        let mut captures = CaptureCoordinator::new();
        let screen_rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(600.0, 550.0));
        let mut run_frame = |events| {
            let _ = context.run(
                RawInput {
                    screen_rect: Some(screen_rect),
                    events,
                    ..Default::default()
                },
                |context| {
                    CentralPanel::default()
                        .frame(egui::Frame::none())
                        .show(context, |ui| {
                            canvas.ui(ui, &mut previews, &mut captures, context, true);
                        });
                },
            );
        };

        let row_center = playlist_first_row_center(
            Rect::from_min_size(Pos2::new(100.0, 100.0), Vec2::new(340.0, 360.0)),
            1.0,
        );
        run_frame(vec![Event::PointerMoved(row_center)]);
        run_frame(vec![Event::PointerButton {
            pos: row_center,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        }]);
        run_frame(vec![Event::PointerButton {
            pos: row_center,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }]);

        assert!(canvas
            .pending_playlist_actions
            .contains(&(id, PlaylistAction::Select(video))));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn playlist_row_stays_clickable_after_canvas_zoom() {
        let root = std::env::temp_dir().join(format!(
            "pluriview-canvas-playlist-zoom-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let video = root.join("01 opening.mp4");
        std::fs::write(&video, b"").unwrap();

        let context = Context::default();
        let mut canvas = CanvasState::default();
        canvas.zoom = 2.0;
        let mut previews = PreviewManager::new();
        let playlist = FolderPlaylist::scan(root.clone(), None).unwrap();
        let id = previews.add_folder_playlist(
            playlist,
            "Test folder".to_owned(),
            Pos2::new(100.0, 100.0),
            Vec2::new(340.0, 360.0),
            1,
            None,
        );
        previews.get_mut(id).unwrap().created_at = Instant::now() - Duration::from_secs(1);
        let mut captures = CaptureCoordinator::new();
        let screen_rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 1100.0));
        let mut run_frame = |events| {
            let _ = context.run(
                RawInput {
                    screen_rect: Some(screen_rect),
                    events,
                    ..Default::default()
                },
                |context| {
                    CentralPanel::default()
                        .frame(egui::Frame::none())
                        .show(context, |ui| {
                            canvas.ui(ui, &mut previews, &mut captures, context, true);
                        });
                },
            );
        };

        let row_center = playlist_first_row_center(
            Rect::from_min_size(Pos2::new(200.0, 200.0), Vec2::new(680.0, 720.0)),
            2.0,
        );
        run_frame(vec![Event::PointerMoved(row_center)]);
        run_frame(vec![Event::PointerButton {
            pos: row_center,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        }]);
        run_frame(vec![Event::PointerButton {
            pos: row_center,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }]);

        assert!(canvas
            .pending_playlist_actions
            .contains(&(id, PlaylistAction::Select(video))));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn focus_fits_tile_and_restores_canvas_view() {
        let mut canvas = CanvasState::default();
        canvas.pan = Vec2::new(21.0, -8.0);
        canvas.zoom = 0.75;
        canvas.selection = vec![PreviewId(9)];
        canvas.animation.momentum_active = true;
        canvas.animation.momentum_velocity = Vec2::splat(10.0);
        let viewport = Rect::from_min_size(Pos2::new(40.0, 30.0), Vec2::new(1200.0, 700.0));
        let tile = Rect::from_min_size(Pos2::new(300.0, 150.0), Vec2::new(800.0, 450.0));

        canvas.focus_on_tile(PreviewId(2), tile, viewport);
        let focused = canvas.canvas_rect_to_screen(tile, viewport);
        assert!((focused.center() - viewport.center()).length() < 0.01);
        assert!((focused.min.x - viewport.min.x).abs() < 0.01);
        assert!((focused.max.x - viewport.max.x).abs() < 0.01);
        assert!(focused.min.y >= viewport.min.y - 0.01);
        assert!(focused.max.y <= viewport.max.y + 0.01);
        assert_eq!(canvas.selection, vec![PreviewId(2)]);
        assert!(!canvas.animation.momentum_active);
        assert_eq!(canvas.animation.momentum_velocity, Vec2::ZERO);
        assert!(canvas.is_focusing_tile());

        assert!(canvas.exit_focus());
        assert!(!canvas.is_focusing_tile());
        assert_eq!(canvas.pan, Vec2::new(21.0, -8.0));
        assert_eq!(canvas.zoom, 0.75);
        assert_eq!(canvas.selection, vec![PreviewId(9)]);
        assert!(!canvas.exit_focus());
    }

    #[test]
    fn focus_fits_tall_tile_without_cropping() {
        let mut canvas = CanvasState::default();
        let viewport = Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 700.0));
        let tile = Rect::from_min_size(Pos2::ZERO, Vec2::new(200.0, 600.0));

        canvas.focus_on_tile(PreviewId(3), tile, viewport);
        let focused = canvas.canvas_rect_to_screen(tile, viewport);

        assert!((focused.center() - viewport.center()).length() < 0.01);
        assert!((focused.min.y - viewport.min.y).abs() < 0.01);
        assert!((focused.max.y - viewport.max.y).abs() < 0.01);
        assert!(focused.min.x >= viewport.min.x - 0.01);
        assert!(focused.max.x <= viewport.max.x + 0.01);
    }

    #[test]
    fn removing_focused_tile_exits_focus_without_restoring_deleted_selection() {
        let mut previews = PreviewManager::new();
        let id = previews.add("focused".to_owned(), Pos2::ZERO, Vec2::splat(100.0));
        let mut canvas = CanvasState::default();
        canvas.selection = vec![id, PreviewId(99)];
        let viewport = Rect::from_min_size(Pos2::ZERO, Vec2::splat(500.0));
        canvas.focus_on_tile(id, previews.get(id).unwrap().rect(), viewport);

        previews.start_removal(id);
        canvas.refit_focus(&previews, viewport);

        assert_eq!(canvas.selection, vec![PreviewId(99)]);
        assert!(!canvas.exit_focus());
    }
}

/// Resize handle positions
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResizeHandle {
    TopLeft,
    Top,
    TopRight,
    Left,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
}

impl ResizeHandle {
    /// Get cursor icon for this handle
    fn cursor(&self) -> CursorIcon {
        match self {
            ResizeHandle::TopLeft | ResizeHandle::BottomRight => CursorIcon::ResizeNwSe,
            ResizeHandle::TopRight | ResizeHandle::BottomLeft => CursorIcon::ResizeNeSw,
            ResizeHandle::Top | ResizeHandle::Bottom => CursorIcon::ResizeVertical,
            ResizeHandle::Left | ResizeHandle::Right => CursorIcon::ResizeHorizontal,
        }
    }
}

/// Pending FPS change (to be applied after UI pass)
#[derive(Clone)]
pub struct PendingFpsChange {
    pub preview_id: PreviewId,
    pub new_fps: FpsPreset,
}

/// Actions requested from a browser tile's hover controls or context menu.
/// The canvas only queues these; the app (which owns the browser hosts)
/// consumes them after the UI pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowserAction {
    Back,
    Forward,
    Reload,
    ToggleMute,
    OpenExternal,
    CopyUrl,
    EditUrl,
}

fn show_context_menu_if(
    response: &egui::Response,
    active: bool,
    add_contents: impl FnOnce(&mut egui::Ui),
) {
    if active {
        let _ = response.context_menu(add_contents);
    }
}

const EGUI_CONTEXT_MENU_ID: &str = "__egui::context_menu";
const SAVED_VIEWPOINTS_SUBMENU_ID: &str = "canvas_saved_viewpoints_submenu";
const SAVED_VIEWPOINTS_SUBMENU_WIDTH: f32 = 280.0;

#[derive(Clone, Copy, Default)]
struct LeftSubmenuState {
    open: bool,
    rect: Option<Rect>,
    parent_rect: Option<Rect>,
    close_parent: bool,
}

fn submenu_should_open_left(parent_right: f32, screen_right: f32, submenu_width: f32) -> bool {
    screen_right - parent_right < submenu_width
}

fn saved_viewpoints_submenu(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui) -> bool) {
    let state_id = egui::Id::new(SAVED_VIEWPOINTS_SUBMENU_ID);
    let open_left = submenu_should_open_left(
        ui.min_rect().right(),
        ui.ctx().screen_rect().right(),
        SAVED_VIEWPOINTS_SUBMENU_WIDTH,
    );
    if !open_left {
        ui.ctx()
            .data_mut(|data| data.insert_temp(state_id, LeftSubmenuState::default()));
        ui.menu_button("Saved Viewpoints", add_contents);
        return;
    }

    let mut state = ui.ctx().data_mut(|data| {
        data.get_temp::<LeftSubmenuState>(state_id)
            .unwrap_or_default()
    });
    let button = ui.button("◀  Saved Viewpoints");
    let pointer = ui.ctx().pointer_hover_pos();
    let hovering_submenu = pointer
        .zip(state.rect)
        .is_some_and(|(pointer, rect)| rect.contains(pointer));
    let crossing_gap = pointer.zip(state.rect).is_some_and(|(pointer, rect)| {
        Rect::from_min_max(
            Pos2::new(rect.right(), button.rect.top()),
            Pos2::new(button.rect.left(), button.rect.bottom()),
        )
        .contains(pointer)
    });

    if button.hovered() {
        state.open = true;
    } else if state.open && !hovering_submenu && !crossing_gap {
        state.open = false;
    }

    state.close_parent = false;
    if state.open {
        let frame_margin = egui::Frame::menu(ui.style()).total_margin();
        let position = Pos2::new(
            button.rect.left() - ui.spacing().menu_spacing,
            button.rect.top() - frame_margin.top,
        );
        let area = egui::Area::new(state_id.with("area"))
            .kind(egui::UiKind::Menu)
            .order(egui::Order::Foreground)
            .pivot(egui::Align2::RIGHT_TOP)
            .fixed_pos(position)
            .default_width(ui.spacing().menu_width)
            .sense(Sense::hover())
            .show(ui.ctx(), |ui| {
                let style = ui.style_mut();
                style.spacing.button_padding = egui::vec2(2.0, 0.0);
                style.visuals.widgets.active.bg_stroke = Stroke::NONE;
                style.visuals.widgets.hovered.bg_stroke = Stroke::NONE;
                style.visuals.widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
                style.visuals.widgets.inactive.bg_stroke = Stroke::NONE;

                egui::Frame::menu(ui.style())
                    .show(ui, |ui| {
                        ui.with_layout(
                            egui::Layout::top_down_justified(egui::Align::LEFT),
                            add_contents,
                        )
                        .inner
                    })
                    .inner
            });
        state.rect = Some(area.response.rect);
        state.close_parent = area.inner;
    } else {
        state.rect = None;
    }

    ui.ctx().data_mut(|data| data.insert_temp(state_id, state));
}

fn show_canvas_context_menu_if(
    response: &egui::Response,
    active: bool,
    add_contents: impl FnOnce(&mut egui::Ui),
) {
    if !active {
        return;
    }

    let state_id = egui::Id::new(SAVED_VIEWPOINTS_SUBMENU_ID);
    let mut state = response.ctx.data_mut(|data| {
        data.get_temp::<LeftSubmenuState>(state_id)
            .unwrap_or_default()
    });
    let menu_id = egui::Id::new(EGUI_CONTEXT_MENU_ID);
    let mut bar_state = egui::menu::BarState::load(&response.ctx, menu_id);
    let menu_response = egui::menu::MenuRoot::context_interaction(response, &mut bar_state);

    if matches!(menu_response, egui::menu::MenuResponse::Create(_, _)) {
        state = LeftSubmenuState::default();
    }
    egui::menu::MenuRoot::handle_menu_response(&mut bar_state, menu_response);

    // The expanded rectangle keeps clicks in the custom left submenu from
    // dismissing the root. Restore the root's real rectangle before drawing,
    // because egui also uses its minimum corner as the popup anchor.
    if let (Some(root), Some(parent_rect)) = (bar_state.as_mut(), state.parent_rect) {
        root.menu_state.write().rect = parent_rect;
    }

    let _ = bar_state.show(response, add_contents);
    state = response.ctx.data_mut(|data| {
        data.get_temp::<LeftSubmenuState>(state_id)
            .unwrap_or_default()
    });

    if state.close_parent {
        egui::menu::MenuRoot::handle_menu_response(&mut bar_state, egui::menu::MenuResponse::Close);
        state = LeftSubmenuState::default();
    } else if let Some(root) = bar_state.as_mut() {
        let parent_rect = root.menu_state.read().rect;
        state.parent_rect = Some(parent_rect);
        if state.open {
            if let Some(submenu_rect) = state.rect {
                root.menu_state.write().rect = parent_rect.union(submenu_rect);
            }
        }
    } else {
        state = LeftSubmenuState::default();
    }

    bar_state.store(&response.ctx, menu_id);
    response
        .ctx
        .data_mut(|data| data.insert_temp(state_id, state));
}

fn browser_control_colors(
    action: BrowserAction,
    hovered: bool,
    muted: bool,
) -> (Option<Color32>, Color32) {
    if matches!(action, BrowserAction::Back | BrowserAction::Forward) {
        let background =
            Color32::from_rgba_unmultiplied(74, 158, 255, if hovered { 130 } else { 70 });
        let icon = if hovered {
            Color32::WHITE
        } else {
            Color32::from_rgb(155, 205, 255)
        };
        return (Some(background), icon);
    }

    let background = hovered.then(|| Color32::from_rgba_unmultiplied(255, 255, 255, 35));
    let icon = if action == BrowserAction::ToggleMute && muted {
        Color32::from_rgb(255, 150, 100)
    } else {
        Color32::from_rgb(215, 215, 220)
    };
    (background, icon)
}

/// Actions requested from an mpv-backed tile. The canvas queues commands and
/// the app forwards them to the session after the UI pass.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum VideoAction {
    SetPaused(bool),
    Reload,
    RequestSeekPreview(f64),
    SeekAbsolute(f64),
    SetVolume(f64),
    ToggleMute,
    SetSpeed(f64),
    ToggleLoop,
    SelectAudioTrack(i64),
    SelectSubtitleTrack(Option<i64>),
    OpenSettings,
}

/// Actions requested by a folder playlist tile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlaylistAction {
    Select(PathBuf),
    Previous,
    Next,
    ToggleAutoplay,
    ToggleShuffle,
    ToggleRepeat,
    Rescan,
    RequestThumbnail(PathBuf),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaAction {
    Retry,
    Locate,
}

/// User-requested runtime activity changes. The canvas queues these and the
/// app applies them because it owns browser hosts, video sessions, and capture
/// workers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TileActivityAction {
    Freeze,
    Resume,
}

/// Snapshot of the input state the canvas actually needs, gathered once per
/// frame instead of cloning the entire egui `InputState` several times.
struct FrameInput {
    hover_pos: Option<Pos2>,
    interact_pos: Option<Pos2>,
    pointer_delta: Vec2,
    scroll_y: f32,
    alt: bool,
    ctrl: bool,
    middle_down: bool,
    primary_down: bool,
    primary_pressed: bool,
    primary_released: bool,
    pointer_blocked: bool,
    time: f64,
    delete_pressed: bool,
    select_all: bool,
    escape_pressed: bool,
}

/// Immutable canvas values shared by the interaction passes for one frame.
#[derive(Clone, Copy)]
struct CanvasFrameScope<'a> {
    canvas_rect: Rect,
    input: &'a FrameInput,
    show_overlays: bool,
}

/// Keyboard actions resolved by the app's configurable hotkey layer. `None`
/// means a direct CanvasState test/embedding, which retains the legacy defaults.
#[derive(Clone, Copy, Debug, Default)]
pub struct CanvasKeyboardInput {
    pub delete_selected: bool,
    pub select_all: bool,
    pub exit_tile_focus: bool,
}

#[derive(Clone, Debug)]
struct MarqueeSelection {
    start: Pos2,
    current: Pos2,
    base_selection: Vec<PreviewId>,
}

#[derive(Clone, Copy, Debug)]
struct PinnedPointerDrag {
    id: PreviewId,
    start_pointer: Pos2,
    start_pin: ViewportPin,
}

impl MarqueeSelection {
    fn rect(&self) -> Rect {
        Rect::from_two_pos(self.start, self.current)
    }

    fn is_drag(&self) -> bool {
        (self.current - self.start).length_sq() >= 16.0
    }
}

#[derive(Clone)]
struct FocusState {
    id: PreviewId,
    pan: Vec2,
    zoom: f32,
    selection: Vec<PreviewId>,
}

/// Per-tile data collected up front so the manager isn't borrowed during
/// the interaction pass.
#[derive(Clone)]
struct TileInfo {
    id: PreviewId,
    z_order: u32,
    rect: Rect,
    title: String,
    target_fps: u32,
    fps_preset: FpsPreset,
    has_crop: bool,
    crop_uv: Option<(f32, f32, f32, f32)>,
    is_removing: bool,
    spawn_t: f32,
    remove_t: f32,
    is_browser: bool,
    is_media: bool,
    is_video: bool,
    is_playlist: bool,
    is_window_capture: bool,
    is_inactive_window: bool,
    window_exe: Option<String>,
    is_spout_capture: bool,
    viewport_pin: Option<ViewportPin>,
    lock_aspect_ratio: bool,
    left_click_disabled: bool,
    muted: bool,
    stream_audio: bool,
    capture_failed: bool,
    browser_status: BrowserTileStatus,
    browser_waiting_for_content: bool,
    video_status: VideoTileStatus,
    video_playback: VideoPlaybackState,
    supports_seek_preview: bool,
    manually_frozen: bool,
    frame_size: Option<(u32, u32)>,
    source_frame_size: Option<(u32, u32)>,
}

impl TileInfo {
    fn from_preview(preview: &Preview) -> Self {
        Self {
            id: preview.id,
            z_order: preview.z_order,
            rect: preview.rect(),
            title: preview.title.clone(),
            target_fps: preview.target_fps,
            fps_preset: preview.fps_preset,
            has_crop: preview.crop_uv.is_some(),
            crop_uv: preview.crop_uv,
            is_removing: preview.removing.is_some(),
            spawn_t: preview.spawn_progress(),
            remove_t: preview.removal_progress(),
            is_browser: preview.is_browser(),
            is_media: preview.is_media(),
            is_video: preview.is_video(),
            is_playlist: preview.is_playlist(),
            is_window_capture: preview.is_window_capture(),
            is_inactive_window: preview.is_inactive_window(),
            window_exe: preview.window_exe.clone(),
            is_spout_capture: preview.is_spout_capture(),
            viewport_pin: preview.viewport_pin,
            lock_aspect_ratio: preview.lock_aspect_ratio,
            left_click_disabled: preview.left_click_disabled,
            muted: preview.browser_muted,
            stream_audio: preview.stream_audio,
            capture_failed: preview.capture_error.is_some(),
            browser_status: preview.browser_status.clone(),
            browser_waiting_for_content: preview.browser_waiting_for_content,
            video_status: preview.video_status.clone(),
            video_playback: preview.video_playback.clone(),
            supports_seek_preview: preview.supports_seek_preview(),
            manually_frozen: preview.manually_frozen,
            frame_size: preview.frame_size,
            source_frame_size: preview.source_frame_size,
        }
    }

    fn update_from(&mut self, preview: &Preview) {
        self.id = preview.id;
        self.z_order = preview.z_order;
        self.rect = preview.rect();
        self.title.clone_from(&preview.title);
        self.target_fps = preview.target_fps;
        self.fps_preset = preview.fps_preset;
        self.has_crop = preview.crop_uv.is_some();
        self.crop_uv = preview.crop_uv;
        self.is_removing = preview.removing.is_some();
        self.spawn_t = preview.spawn_progress();
        self.remove_t = preview.removal_progress();
        self.is_browser = preview.is_browser();
        self.is_media = preview.is_media();
        self.is_video = preview.is_video();
        self.is_playlist = preview.is_playlist();
        self.is_window_capture = preview.is_window_capture();
        self.is_inactive_window = preview.is_inactive_window();
        self.window_exe.clone_from(&preview.window_exe);
        self.is_spout_capture = preview.is_spout_capture();
        self.viewport_pin = preview.viewport_pin;
        self.lock_aspect_ratio = preview.lock_aspect_ratio;
        self.left_click_disabled = preview.left_click_disabled;
        self.muted = preview.browser_muted;
        self.stream_audio = preview.stream_audio;
        self.capture_failed = preview.capture_error.is_some();
        self.browser_status.clone_from(&preview.browser_status);
        self.browser_waiting_for_content = preview.browser_waiting_for_content;
        self.video_status.clone_from(&preview.video_status);
        self.video_playback.clone_from(&preview.video_playback);
        self.supports_seek_preview = preview.supports_seek_preview();
        self.manually_frozen = preview.manually_frozen;
        self.frame_size = preview.frame_size;
        self.source_frame_size = preview.source_frame_size;
    }
}

fn format_time(seconds: Option<f64>) -> String {
    let seconds = seconds
        .filter(|value| value.is_finite())
        .unwrap_or(0.0)
        .max(0.0) as u64;
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let seconds = seconds % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

fn video_volume_popover_rect(tile_rect: Rect, mute_rect: Rect) -> Rect {
    const WIDTH: f32 = 36.0;
    const HEIGHT: f32 = 116.0;
    const TOP_MARGIN: f32 = 4.0;
    const SIDE_MARGIN: f32 = 3.0;

    let left_limit = tile_rect.left() + SIDE_MARGIN;
    let right_limit = (tile_rect.right() - WIDTH - SIDE_MARGIN).max(left_limit);
    let left = (mute_rect.center().x - WIDTH * 0.5).clamp(left_limit, right_limit);
    let bottom = mute_rect.top() - 6.0;
    let top = (bottom - HEIGHT).max(tile_rect.top() + TOP_MARGIN);
    Rect::from_min_max(Pos2::new(left, top), Pos2::new(left + WIDTH, bottom))
}

fn video_volume_slider_rect(popover: Rect) -> Rect {
    Rect::from_min_max(
        Pos2::new(popover.left() + 8.0, popover.top() + 24.0),
        popover.right_bottom() - Vec2::new(8.0, 8.0),
    )
}

fn video_time_style(tile_width: f32) -> (f32, f32) {
    if tile_width >= 360.0 {
        (108.0, 12.0)
    } else {
        (82.0, 10.0)
    }
}

fn playlist_zoom(zoom: f32) -> f32 {
    zoom.max(0.05)
}

fn playlist_px(value: f32, zoom: f32) -> f32 {
    value * playlist_zoom(zoom)
}

fn playlist_font(size: f32, zoom: f32) -> egui::FontId {
    egui::FontId::proportional(playlist_px(size, zoom).max(1.0))
}

#[cfg(test)]
fn playlist_chrome_height(zoom: f32) -> f32 {
    playlist_px(PLAYLIST_HEADER_HEIGHT + PLAYLIST_TOOLBAR_HEIGHT, zoom)
}

#[cfg(test)]
fn playlist_first_row_center(tile: Rect, zoom: f32) -> Pos2 {
    let row_top = tile.min.y + playlist_chrome_height(zoom);
    Pos2::new(
        tile.center().x,
        row_top + playlist_px(PLAYLIST_ROW_HEIGHT, zoom) * 0.5,
    )
}

fn playlist_entry_labels(name: &str) -> (&str, Option<&str>) {
    match name.rsplit_once('.') {
        Some((stem, ext))
            if !stem.is_empty()
                && ext.len() <= 5
                && ext
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric()) =>
        {
            (stem, Some(ext))
        }
        _ => (name, None),
    }
}

fn cover_uv(image: Vec2, dest: Vec2) -> Rect {
    if image.x <= 0.0 || image.y <= 0.0 || dest.x <= 0.0 || dest.y <= 0.0 {
        return Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
    }
    let image_aspect = image.x / image.y;
    let dest_aspect = dest.x / dest.y;
    if image_aspect > dest_aspect {
        let visible = dest_aspect / image_aspect;
        let pad = (1.0 - visible) * 0.5;
        Rect::from_min_max(Pos2::new(pad, 0.0), Pos2::new(1.0 - pad, 1.0))
    } else {
        let visible = image_aspect / dest_aspect;
        let pad = (1.0 - visible) * 0.5;
        Rect::from_min_max(Pos2::new(0.0, pad), Pos2::new(1.0, 1.0 - pad))
    }
}

fn pixel_aligned_rect(rect: Rect, pixels_per_point: f32) -> Rect {
    let scale = pixels_per_point.max(0.01);
    let snap = |value: f32| (value * scale).round() / scale;
    Rect::from_min_max(
        Pos2::new(snap(rect.min.x), snap(rect.min.y)),
        Pos2::new(snap(rect.max.x), snap(rect.max.y)),
    )
}

fn native_capture_canvas_size(
    source_size: (u32, u32),
    crop_uv: Option<(f32, f32, f32, f32)>,
    pixels_per_point: f32,
    canvas_zoom: f32,
) -> Vec2 {
    let (u_span, v_span) = crop_uv
        .map(|(min_u, min_v, max_u, max_v)| {
            (
                (max_u - min_u).clamp(0.0, 1.0),
                (max_v - min_v).clamp(0.0, 1.0),
            )
        })
        .unwrap_or((1.0, 1.0));
    let width_px = (source_size.0 as f32 * u_span).round().max(1.0);
    let height_px = (source_size.1 as f32 * v_span).round().max(1.0);
    let screen_scale = pixels_per_point.max(0.01) * canvas_zoom.max(0.01);
    Vec2::new(width_px / screen_scale, height_px / screen_scale)
}

fn live_capture_display_size(preview: &Preview, canvas_lod: f32) -> Vec2 {
    preview
        .viewport_pin
        .map(ViewportPin::size_vec2)
        .unwrap_or(preview.size * canvas_lod)
}

fn paint_truncated_text(
    painter: &egui::Painter,
    pos: Pos2,
    anchor: egui::Align2,
    text: &str,
    font_id: egui::FontId,
    color: Color32,
    max_width: f32,
) {
    if text.is_empty() || max_width <= 1.0 {
        return;
    }
    let mut job = egui::text::LayoutJob::simple(text.to_owned(), font_id, color, max_width);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some('…');
    let galley = painter.layout_job(job);
    painter.galley(anchor.anchor_size(pos, galley.size()).min, galley, color);
}

/// Hover capture-resolution pill sits 4px left of FPS, or of the Stream Audio badge
/// when that is shown beside FPS.
const CAPTURE_RESOLUTION_BADGE_WIDTH: f32 = 70.0;

fn capture_resolution_badge_rect(left_of: Rect) -> Rect {
    Rect::from_min_size(
        Pos2::new(
            left_of.left() - 4.0 - CAPTURE_RESOLUTION_BADGE_WIDTH,
            left_of.top(),
        ),
        Vec2::new(CAPTURE_RESOLUTION_BADGE_WIDTH, left_of.height()),
    )
}

/// Hover FPS pill sits at x=-72 with width 36; Stream Audio is 4px to its left.
fn stream_audio_badge_rect(screen_rect: Rect, enabled: bool, beside_fps: bool) -> Rect {
    let width = if enabled { 104.0 } else { 82.0 };
    let x = if beside_fps {
        -76.0 - width
    } else {
        -8.0 - width
    };
    Rect::from_min_size(
        screen_rect.right_top() + Vec2::new(x, 10.0),
        Vec2::new(width, 20.0),
    )
}

fn stream_audio_tooltip(enabled: bool, monitor_ready: bool) -> &'static str {
    match (enabled, monitor_ready) {
        (true, true) => "Streaming audio — click to stop",
        (true, false) => "Stream Audio is on — pick a device under View → Stream Audio Monitor",
        (false, true) => "Stream audio to Discord/OBS",
        (false, false) => "Pick a Stream Audio Monitor device in View first",
    }
}

fn video_status_needs_settings(status: &VideoTileStatus) -> bool {
    let VideoTileStatus::Failed(error) = status else {
        return false;
    };
    let error = error.to_ascii_lowercase();
    error.contains("was not found")
        || error.contains(" is invalid")
        || error.contains("configure it in settings")
}

fn video_placeholder_content(status: &VideoTileStatus) -> (&'static str, &'static str, bool) {
    match status {
        VideoTileStatus::Starting | VideoTileStatus::Ready => {
            ("Starting video", "Waiting for the first frame", true)
        }
        VideoTileStatus::PausedOnRestore => ("Video paused", "Press play to resume", false),
        VideoTileStatus::Buffering => ("Buffering video", "Waiting for the stream", true),
        VideoTileStatus::Failed(_) if video_status_needs_settings(status) => (
            "Video tools unavailable",
            "Open Settings to configure playback",
            false,
        ),
        VideoTileStatus::Failed(_) => {
            ("Video unavailable", "Check the source and try again", false)
        }
    }
}

fn window_capture_placeholder_content(failed: bool) -> (&'static str, &'static str, bool) {
    if failed {
        ("Capture failed", "This window could not be captured", false)
    } else {
        ("Connecting...", "", true)
    }
}

fn media_placeholder_content() -> (&'static str, &'static str) {
    (
        "Image unavailable",
        "Right-click to retry or locate the original file",
    )
}

fn paint_missing_media_placeholder(painter: &egui::Painter, rect: Rect) {
    let (title, detail) = media_placeholder_content();
    painter.rect_filled(rect, 8.0, Color32::from_rgb(24, 20, 20));
    painter.text(
        rect.center() + Vec2::new(0.0, -9.0),
        egui::Align2::CENTER_CENTER,
        title,
        egui::FontId::proportional(13.0),
        Color32::from_rgb(225, 130, 110),
    );
    if rect.height() >= 72.0 {
        painter.text(
            rect.center() + Vec2::new(0.0, 12.0),
            egui::Align2::CENTER_CENTER,
            detail,
            egui::FontId::proportional(10.5),
            Color32::from_rgb(160, 125, 118),
        );
    }
}

fn paint_window_capture_placeholder(
    painter: &egui::Painter,
    rect: Rect,
    failed: bool,
    time: f32,
) -> bool {
    let (title, detail, animated) = window_capture_placeholder_content(failed);
    if failed {
        painter.rect_filled(rect, 8.0, Color32::from_rgb(22, 18, 18));
        painter.text(
            rect.center() + Vec2::new(0.0, if detail.is_empty() { 0.0 } else { -8.0 }),
            egui::Align2::CENTER_CENTER,
            title,
            egui::FontId::proportional(12.0),
            Color32::from_rgb(225, 112, 96),
        );
        if rect.height() >= 80.0 && !detail.is_empty() {
            painter.text(
                rect.center() + Vec2::new(0.0, 12.0),
                egui::Align2::CENTER_CENTER,
                detail,
                egui::FontId::proportional(11.0),
                Color32::from_rgb(140, 110, 105),
            );
        }
        false
    } else {
        let pulse = (time * 1.8).sin() * 0.5 + 0.5;
        let v = (18.0 + pulse * 14.0) as u8;
        painter.rect_filled(rect, 8.0, Color32::from_rgb(v, v, v + 2));
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            title,
            egui::FontId::proportional(12.0),
            Color32::from_rgb(95, 95, 95),
        );
        animated
    }
}

fn paint_inactive_window_placeholder(
    painter: &egui::Painter,
    rect: Rect,
    window_exe: Option<&str>,
) {
    painter.rect_filled(rect, 8.0, Color32::from_rgb(20, 22, 25));
    let center = rect.center();
    painter.text(
        center + Vec2::new(0.0, -8.0),
        egui::Align2::CENTER_CENTER,
        "Not active",
        egui::FontId::proportional(13.0),
        Color32::from_rgb(175, 180, 188),
    );
    if rect.height() >= 64.0 {
        let detail = window_exe
            .filter(|name| !name.is_empty() && !name.eq_ignore_ascii_case("unknown"))
            .map(|name| format!("Waiting for {name} to open"))
            .unwrap_or_else(|| "Waiting for the window to open".to_owned());
        painter.text(
            center + Vec2::new(0.0, 12.0),
            egui::Align2::CENTER_CENTER,
            detail,
            egui::FontId::proportional(10.5),
            Color32::from_rgb(105, 112, 122),
        );
    }
}

fn paint_video_placeholder(
    painter: &egui::Painter,
    rect: Rect,
    status: &VideoTileStatus,
    time: f32,
) -> bool {
    let (title, detail, animated) = video_placeholder_content(status);
    let accent = if matches!(status, VideoTileStatus::Failed(_)) {
        Color32::from_rgb(225, 112, 96)
    } else {
        Color32::from_rgb(74, 158, 255)
    };
    painter.rect_filled(rect, 8.0, Color32::from_rgb(13, 15, 18));

    if animated {
        let pulse = (time * 2.0).sin() * 0.5 + 0.5;
        painter.circle_stroke(
            rect.center() + Vec2::new(0.0, -28.0),
            22.0 + pulse * 2.0,
            Stroke::new(
                2.0,
                Color32::from_rgba_unmultiplied(
                    accent.r(),
                    accent.g(),
                    accent.b(),
                    (90.0 + pulse * 80.0) as u8,
                ),
            ),
        );
    } else {
        painter.circle_stroke(
            rect.center() + Vec2::new(0.0, -28.0),
            22.0,
            Stroke::new(
                1.0,
                Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 110),
            ),
        );
    }
    painter.text(
        rect.center() + Vec2::new(0.0, -28.0),
        egui::Align2::CENTER_CENTER,
        if matches!(status, VideoTileStatus::Failed(_)) {
            egui_phosphor::regular::WARNING
        } else if matches!(status, VideoTileStatus::PausedOnRestore) {
            egui_phosphor::regular::PAUSE
        } else {
            egui_phosphor::regular::VIDEO
        },
        egui::FontId::proportional(20.0),
        accent,
    );
    painter.text(
        rect.center() + Vec2::new(0.0, 8.0),
        egui::Align2::CENTER_CENTER,
        title,
        egui::FontId::proportional(14.0),
        Color32::from_rgb(224, 228, 234),
    );
    if rect.height() >= 145.0 {
        painter.text(
            rect.center() + Vec2::new(0.0, 30.0),
            egui::Align2::CENTER_CENTER,
            detail,
            egui::FontId::proportional(11.0),
            Color32::from_rgb(128, 136, 148),
        );
    }
    animated
}

fn paint_browser_placeholder(
    painter: &egui::Painter,
    rect: Rect,
    status: &BrowserTileStatus,
    time: f32,
) {
    let accent = Color32::from_rgb(207, 161, 57);
    painter.rect_filled(rect, 8.0, Color32::from_rgb(14, 17, 15));

    // A restrained moving glow keeps the tile feeling alive without making
    // the loading state visually noisy.
    let sweep = ((time * 0.16).fract() * 1.5 - 0.25) * rect.width();
    let glow = Rect::from_center_size(
        Pos2::new(rect.left() + sweep, rect.center().y),
        Vec2::new((rect.width() * 0.24).max(36.0), rect.height()),
    )
    .intersect(rect);
    painter.rect_filled(glow, 8.0, Color32::from_rgba_unmultiplied(207, 161, 57, 10));

    let (title, detail, progress, determinate, icon, color) = match status {
        BrowserTileStatus::PreparingAdblock { progress } => (
            "Preparing privacy protection",
            "Installing ad-blocking files",
            progress.clamp(0.0, 1.0),
            true,
            egui_phosphor::regular::SHIELD_CHECK,
            accent,
        ),
        BrowserTileStatus::Starting => (
            "Starting browser preview",
            "Almost ready",
            1.0,
            true,
            egui_phosphor::regular::GLOBE,
            accent,
        ),
        BrowserTileStatus::Failed(_) => (
            "Browser preview unavailable",
            "Restart Pluriview to try again",
            0.0,
            false,
            egui_phosphor::regular::WARNING,
            Color32::from_rgb(225, 112, 96),
        ),
        BrowserTileStatus::Ready => (
            "Connecting browser preview",
            "Waiting for the first frame",
            ((time * 0.35).sin() * 0.15 + 0.75).clamp(0.0, 1.0),
            false,
            egui_phosphor::regular::GLOBE,
            accent,
        ),
    };

    let compact = rect.height() < 170.0;
    let center = rect.center();
    let icon_y = center.y - if compact { 28.0 } else { 50.0 };
    painter.circle_filled(
        Pos2::new(center.x, icon_y),
        if compact { 18.0 } else { 23.0 },
        Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 24),
    );
    painter.circle_stroke(
        Pos2::new(center.x, icon_y),
        if compact { 18.0 } else { 23.0 },
        Stroke::new(
            1.0,
            Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 75),
        ),
    );
    painter.text(
        Pos2::new(center.x, icon_y),
        egui::Align2::CENTER_CENTER,
        icon,
        egui::FontId::proportional(if compact { 17.0 } else { 21.0 }),
        color,
    );

    let title_y = center.y + if compact { 1.0 } else { -8.0 };
    painter.text(
        Pos2::new(center.x, title_y),
        egui::Align2::CENTER_CENTER,
        title,
        egui::FontId::proportional(if compact { 12.0 } else { 14.0 }),
        Color32::from_rgb(224, 228, 224),
    );
    if !compact {
        painter.text(
            Pos2::new(center.x, title_y + 22.0),
            egui::Align2::CENTER_CENTER,
            detail,
            egui::FontId::proportional(11.0),
            Color32::from_rgb(128, 136, 130),
        );
    }

    if !matches!(status, BrowserTileStatus::Failed(_)) {
        let bar_width = (rect.width() - 72.0).clamp(60.0, 320.0);
        let bar_rect = Rect::from_center_size(
            Pos2::new(center.x, title_y + if compact { 27.0 } else { 52.0 }),
            Vec2::new(bar_width, 6.0),
        );
        painter.rect_filled(bar_rect, 3.0, Color32::from_rgb(35, 41, 36));
        let fill_width = (bar_rect.width() * progress).max(if progress > 0.0 { 6.0 } else { 0.0 });
        let fill_rect = Rect::from_min_size(bar_rect.min, Vec2::new(fill_width, bar_rect.height()));
        painter.rect_filled(fill_rect, 3.0, color);
        painter.circle_filled(
            Pos2::new(fill_rect.right(), fill_rect.center().y),
            4.0 + ((time * 3.0).sin() * 0.5 + 0.5),
            Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 150),
        );
        if !compact && determinate {
            painter.text(
                Pos2::new(center.x, bar_rect.bottom() + 15.0),
                egui::Align2::CENTER_CENTER,
                format!("{}%", (progress * 100.0).round() as u32),
                egui::FontId::monospace(10.0),
                Color32::from_rgb(112, 143, 115),
            );
        }
    }
}

/// Canvas state managing pan, zoom, and interactions
#[derive(Clone)]
pub struct CanvasState {
    /// Current pan offset (in canvas coordinates)
    pub pan: Vec2,

    /// Current zoom level (1.0 = 100%)
    pub zoom: f32,

    /// Zoom constraints
    pub zoom_min: f32,
    pub zoom_max: f32,

    /// Currently selected preview IDs
    pub selection: Vec<PreviewId>,

    /// Current drag operation
    pub drag_state: Option<DragState>,

    /// Grid visibility
    pub show_grid: bool,

    /// Grid size in canvas units
    pub grid_size: f32,

    /// Screen-space live wallpaper drawn behind tiles. Pan and zoom never
    /// move or scale it.
    pub wallpaper: Option<CanvasWallpaper>,

    /// Open the wallpaper file picker after the context/menu closes.
    pub pending_wallpaper_pick: bool,

    /// Drop the current wallpaper after the context/menu closes.
    pub pending_wallpaper_clear: bool,

    /// Pending FPS changes to apply
    pending_fps_changes: Vec<PendingFpsChange>,

    /// Window-capture tiles whose SA toggle was clicked this frame.
    pending_stream_audio_toggles: Vec<PreviewId>,

    /// Animation state for smooth movements
    pub animation: AnimationState,

    /// Reused visible-tile snapshot storage for the draw/interaction pass.
    tile_scratch: Vec<TileInfo>,

    /// Is a preview currently being dragged?
    preview_dragging: bool,

    /// Original viewport placements for pinned tiles in the active
    /// drag, paired with their IDs so egui's cumulative drag delta is stable.
    pinned_drag_origins: Vec<(PreviewId, ViewportPin)>,

    /// Direct left-button capture for repositioning one pinned tile.
    /// This does not depend on egui's general tile-drag ownership.
    pinned_pointer_drag: Option<PinnedPointerDrag>,

    /// Is the canvas currently being panned?
    canvas_panning: bool,

    /// Drag tracker for canvas pan momentum
    pan_drag_tracker: DragTracker,

    /// Preview ID pending region selection (set from context menu, consumed by app)
    pub pending_region_select: Option<PreviewId>,

    /// Latest deletion batch, retained until undo, another deletion, or workspace change.
    last_removed: Option<(Instant, Vec<RemovedPreviewInfo>)>,
    removed_undo_requested: bool,

    /// Screen position of the last right-click on the canvas background,
    /// used to anchor the "Add Window..." quick-add popup.
    last_secondary_click: Option<Pos2>,

    /// Set by the "Add Window..." context menu item: (canvas position to
    /// place the new preview, screen position to anchor the popup). The app
    /// consumes this to open the quick-add popup.
    pub pending_quick_add: Option<(Pos2, Pos2)>,

    /// Canvas position requested by the "Add Browser..." context action.
    pub pending_browser_add: Option<Pos2>,

    /// Canvas position requested by the "Add Image..." context action.
    pub pending_media_add: Option<Pos2>,

    /// Canvas position requested by the "Add Video..." context action.
    pub pending_video_add: Option<Pos2>,

    /// Canvas position requested by the "Add Stream..." context action.
    pub pending_stream_add: Option<Pos2>,

    /// Browser tile actions queued by hover controls / context menus,
    /// consumed by the app.
    pub pending_browser_actions: Vec<(PreviewId, BrowserAction)>,

    /// Video tile actions queued by hover controls / context menus.
    pub pending_video_actions: Vec<(PreviewId, VideoAction)>,

    /// Video tile whose speaker control currently owns the hover volume slider.
    video_volume_hover: Option<PreviewId>,

    /// Freeze/resume requests queued by tile or background context menus.
    pub pending_tile_activity_actions: Vec<(PreviewId, TileActivityAction)>,

    /// Folder playlist actions queued by its rows and header controls.
    pub pending_playlist_actions: Vec<(PreviewId, PlaylistAction)>,

    /// Removed batch requested by button or shortcut. The app owns source recreation
    /// so every tile type follows one restore path.
    pub pending_removed_restore: Option<Vec<RemovedPreviewInfo>>,
    pub pending_media_actions: Vec<(PreviewId, MediaAction)>,

    /// The browser tile currently in interaction mode, set by the app each
    /// frame so the canvas can outline it in the accent color.
    pub interactive_browser: Option<PreviewId>,

    /// True when View → Stream Audio Monitor has a target device.
    pub stream_monitor_ready: bool,

    /// Last canvas rectangle in egui screen coordinates.
    pub last_screen_rect: Option<Rect>,

    /// Preview most recently double-clicked, consumed by the app.
    pub last_double_clicked: Option<PreviewId>,

    /// Canvas view to restore when the user exits temporary tile focus.
    focus: Option<FocusState>,

    /// Left-button selection box started on empty canvas.
    marquee: Option<MarqueeSelection>,

    /// Configurable keyboard actions queued by the app for the next UI pass.
    keyboard_input: Option<CanvasKeyboardInput>,
}

impl Default for CanvasState {
    fn default() -> Self {
        Self {
            pan: Vec2::ZERO,
            zoom: 1.0,
            zoom_min: 0.1,
            zoom_max: 5.0,
            selection: Vec::new(),
            drag_state: None,
            show_grid: true,
            grid_size: 50.0,
            wallpaper: None,
            pending_wallpaper_pick: false,
            pending_wallpaper_clear: false,
            pending_fps_changes: Vec::new(),
            pending_stream_audio_toggles: Vec::new(),
            animation: AnimationState::new(),
            tile_scratch: Vec::new(),
            preview_dragging: false,
            pinned_drag_origins: Vec::new(),
            pinned_pointer_drag: None,
            canvas_panning: false,
            pan_drag_tracker: DragTracker::new(),
            pending_region_select: None,
            last_removed: None,
            removed_undo_requested: false,
            last_secondary_click: None,
            pending_quick_add: None,
            pending_browser_add: None,
            pending_media_add: None,
            pending_video_add: None,
            pending_stream_add: None,
            pending_browser_actions: Vec::new(),
            pending_video_actions: Vec::new(),
            video_volume_hover: None,
            pending_tile_activity_actions: Vec::new(),
            pending_playlist_actions: Vec::new(),
            pending_removed_restore: None,
            pending_media_actions: Vec::new(),
            interactive_browser: None,
            stream_monitor_ready: false,
            last_screen_rect: None,
            last_double_clicked: None,
            focus: None,
            marquee: None,
            keyboard_input: None,
        }
    }
}

impl CanvasState {
    pub fn set_keyboard_input(&mut self, input: CanvasKeyboardInput) {
        self.keyboard_input = Some(input);
    }

    /// Reset canvas to default view
    pub fn reset(&mut self) {
        self.pan = Vec2::ZERO;
        self.zoom = 1.0;
        self.selection.clear();
        self.drag_state = None;
        self.marquee = None;
        self.pinned_drag_origins.clear();
        self.pinned_pointer_drag = None;
        self.video_volume_hover = None;
        self.cancel_camera_transition();
    }

    /// Drop transient tile interaction state from a previous layout.
    pub fn clear_preview_interactions(&mut self) {
        self.last_removed = None;
        self.removed_undo_requested = false;
        self.pending_removed_restore = None;
        self.pending_media_actions.clear();
        self.pinned_drag_origins.clear();
        self.pinned_pointer_drag = None;
        self.video_volume_hover = None;
    }

    fn prune_preview_interactions(&mut self, preview_manager: &PreviewManager) {
        if self
            .pinned_pointer_drag
            .is_some_and(|drag| preview_manager.get(drag.id).is_none())
        {
            self.pinned_pointer_drag = None;
        }
    }

    /// Fit one tile in the current canvas without changing its saved geometry.
    pub fn focus_on_tile(&mut self, id: PreviewId, tile_rect: Rect, canvas_rect: Rect) {
        self.cancel_camera_transition();
        if let Some(focus) = &mut self.focus {
            focus.id = id;
        } else {
            self.focus = Some(FocusState {
                id,
                pan: self.pan,
                zoom: self.zoom,
                selection: self.selection.clone(),
            });
        }

        let available = canvas_rect.size().max(Vec2::splat(1.0));
        let tile_size = tile_rect.size().max(Vec2::splat(1.0));
        self.zoom = (available.x / tile_size.x)
            .min(available.y / tile_size.y)
            .clamp(self.zoom_min, self.zoom_max);
        self.pan =
            (canvas_rect.center() - canvas_rect.min) / self.zoom - tile_rect.center().to_vec2();
        self.selection = vec![id];
        self.animation.momentum_active = false;
        self.animation.momentum_velocity = Vec2::ZERO;
    }

    pub fn is_focusing_tile(&self) -> bool {
        self.focus.is_some()
    }

    /// Canvas-space point currently shown at the center of the viewport.
    pub fn camera_center(&self, canvas_rect: Rect) -> Pos2 {
        self.screen_to_canvas(canvas_rect.center(), canvas_rect)
    }

    /// Glide to a named view. A new request samples the camera's current
    /// position, so selecting another view mid-transition remains continuous.
    pub fn start_camera_transition(
        &mut self,
        target_center: Pos2,
        target_zoom: f32,
        canvas_rect: Rect,
        now: f64,
    ) {
        let start_center = self.camera_center(canvas_rect);
        let target_zoom = target_zoom.clamp(self.zoom_min, self.zoom_max);
        self.focus = None;
        self.animation.momentum_active = false;
        self.animation.momentum_velocity = Vec2::ZERO;

        if start_center.distance(target_center) < 0.01 && (self.zoom - target_zoom).abs() < 0.0001 {
            self.zoom = target_zoom;
            self.pan =
                (canvas_rect.center() - canvas_rect.min) / self.zoom - target_center.to_vec2();
            self.animation.camera_transition = None;
            return;
        }

        self.animation.camera_transition = Some(CameraTransition::new(
            start_center,
            target_center,
            self.zoom,
            target_zoom,
            canvas_rect.size(),
            now,
        ));
    }

    /// Stop a saved-view glide at its current position. Direct navigation
    /// input calls this before applying its own movement.
    pub fn cancel_camera_transition(&mut self) {
        self.animation.camera_transition = None;
    }

    pub fn is_camera_transitioning(&self) -> bool {
        self.animation.camera_transition.is_some()
    }

    fn update_camera_transition(&mut self, canvas_rect: Rect, now: f64) {
        let Some(sample) = self
            .animation
            .camera_transition
            .as_ref()
            .map(|transition| transition.sample(now))
        else {
            return;
        };

        self.zoom = sample.zoom.clamp(self.zoom_min, self.zoom_max);
        self.pan = (canvas_rect.center() - canvas_rect.min) / self.zoom - sample.center.to_vec2();
        if sample.finished {
            self.animation.camera_transition = None;
        }
    }

    /// Restore the canvas view saved before tile focus.
    pub fn exit_focus(&mut self) -> bool {
        let Some(focus) = self.focus.take() else {
            return false;
        };
        self.pan = focus.pan;
        self.zoom = focus.zoom;
        self.selection = focus.selection;
        true
    }

    fn refit_focus(&mut self, preview_manager: &PreviewManager, canvas_rect: Rect) {
        let Some(id) = self.focus.as_ref().map(|focus| focus.id) else {
            return;
        };
        if let Some(tile_rect) = preview_manager
            .get(id)
            .filter(|preview| preview.removing.is_none() && preview.viewport_pin.is_none())
            .map(|preview| preview.rect())
        {
            self.focus_on_tile(id, tile_rect, canvas_rect);
        } else {
            self.exit_focus();
            self.selection.retain(|selected| *selected != id);
        }
    }

    /// Convert screen position to canvas position
    pub fn screen_to_canvas(&self, screen_pos: Pos2, canvas_rect: Rect) -> Pos2 {
        let relative = screen_pos - canvas_rect.min;
        Pos2::new(
            (relative.x / self.zoom) - self.pan.x,
            (relative.y / self.zoom) - self.pan.y,
        )
    }

    /// Convert canvas position to screen position
    pub fn canvas_to_screen(&self, canvas_pos: Pos2, canvas_rect: Rect) -> Pos2 {
        Pos2::new(
            (canvas_pos.x + self.pan.x) * self.zoom + canvas_rect.min.x,
            (canvas_pos.y + self.pan.y) * self.zoom + canvas_rect.min.y,
        )
    }

    /// Convert canvas rect to screen rect
    pub fn canvas_rect_to_screen(&self, canvas_rect: Rect, screen_canvas_rect: Rect) -> Rect {
        let min = self.canvas_to_screen(canvas_rect.min, screen_canvas_rect);
        let max = self.canvas_to_screen(canvas_rect.max, screen_canvas_rect);
        Rect::from_min_max(min, max)
    }

    fn preview_screen_rect(&self, preview: &Preview, canvas_rect: Rect) -> Rect {
        preview
            .viewport_pin
            .map(|pin| pin.rect(canvas_rect))
            .unwrap_or_else(|| self.canvas_rect_to_screen(preview.rect(), canvas_rect))
    }

    /// Apply or clear crop UVs while changing the tile bounds by the same
    /// source-space proportion. This keeps content scale stable for region
    /// selection and Clear Crop as well as handle dragging.
    pub fn set_preview_crop(
        &mut self,
        id: PreviewId,
        crop_uv: Option<(f32, f32, f32, f32)>,
        canvas_rect: Rect,
        preview_manager: &mut PreviewManager,
    ) {
        let Some((current_crop, current_rect, screen_space, source_size)) =
            preview_manager.get(id).map(|preview| {
                let screen_space = preview.viewport_pin.is_some();
                (
                    preview.crop_uv.unwrap_or((0.0, 0.0, 1.0, 1.0)),
                    if screen_space {
                        self.preview_screen_rect(preview, canvas_rect)
                    } else {
                        preview.rect()
                    },
                    screen_space,
                    preview.source_frame_size.or(preview.frame_size),
                )
            })
        else {
            return;
        };
        let target_crop = crop_uv.unwrap_or((0.0, 0.0, 1.0, 1.0));
        let new_rect = rect_for_crop_change(current_crop, target_crop, current_rect);
        if let Some(preview) = preview_manager.get_mut(id) {
            preview.crop_uv = crop_uv;
            if screen_space {
                preview.viewport_pin = Some(ViewportPin::from_rect(new_rect, canvas_rect));
            } else {
                preview.position = new_rect.min;
                preview.size = new_rect.size();
            }
            if let Some((width, height)) = source_size {
                let cropped_width = (target_crop.2 - target_crop.0) * width as f32;
                let cropped_height = (target_crop.3 - target_crop.1) * height as f32;
                if cropped_height > 0.0 {
                    preview.source_aspect_ratio = cropped_width / cropped_height;
                }
            } else if new_rect.height() > 0.0 {
                preview.source_aspect_ratio = new_rect.width() / new_rect.height();
            }
        }
    }

    fn toggle_viewport_pin(
        &mut self,
        id: PreviewId,
        canvas_rect: Rect,
        preview_manager: &mut PreviewManager,
    ) {
        let Some((pin, canvas_preview_rect)) = preview_manager
            .get(id)
            .map(|preview| (preview.viewport_pin, preview.rect()))
        else {
            return;
        };

        self.exit_focus();
        self.pinned_drag_origins
            .retain(|(drag_id, _)| *drag_id != id);
        if self.pinned_pointer_drag.is_some_and(|drag| drag.id == id) {
            self.pinned_pointer_drag = None;
        }

        if let Some(pin) = pin {
            let screen_rect = pin.rect(canvas_rect);
            let min = self.screen_to_canvas(screen_rect.min, canvas_rect);
            let max = self.screen_to_canvas(screen_rect.max, canvas_rect);
            if let Some(preview) = preview_manager.get_mut(id) {
                preview.position = min;
                preview.size = (max - min).max(Vec2::splat(1.0));
                preview.viewport_pin = None;
            }
        } else {
            let screen_rect = self.canvas_rect_to_screen(canvas_preview_rect, canvas_rect);
            if let Some(preview) = preview_manager.get_mut(id) {
                preview.viewport_pin = Some(ViewportPin::from_rect(screen_rect, canvas_rect));
            }
        }
    }

    fn toggle_left_click_disabled(&mut self, id: PreviewId, preview_manager: &mut PreviewManager) {
        let Some(disabled) = preview_manager.get_mut(id).map(|preview| {
            preview.left_click_disabled = !preview.left_click_disabled;
            preview.left_click_disabled
        }) else {
            return;
        };

        if disabled {
            if self.focus.as_ref().is_some_and(|focus| focus.id == id) {
                self.exit_focus();
            }
            self.selection.retain(|selected| *selected != id);
            self.pinned_drag_origins
                .retain(|(drag_id, _)| *drag_id != id);
            if self.pinned_pointer_drag.is_some_and(|drag| drag.id == id) {
                self.pinned_pointer_drag = None;
            }
            if self.interactive_browser == Some(id) {
                // The app consumes this runtime request and parks the native
                // WebView host so it cannot continue intercepting pointer input.
                self.last_double_clicked = Some(id);
            }
        }
    }

    fn toggle_aspect_ratio_lock(&mut self, id: PreviewId, preview_manager: &mut PreviewManager) {
        let Some(preview) = preview_manager.get_mut(id) else {
            return;
        };
        preview.lock_aspect_ratio = !preview.lock_aspect_ratio;
        if preview.lock_aspect_ratio {
            let size = preview
                .viewport_pin
                .map(ViewportPin::size_vec2)
                .unwrap_or(preview.size);
            if size.y > 0.0 {
                preview.source_aspect_ratio = size.x / size.y;
            }
        }
    }

    fn set_native_capture_size(
        &mut self,
        id: PreviewId,
        source_size: (u32, u32),
        canvas_rect: Rect,
        pixels_per_point: f32,
        preview_manager: &mut PreviewManager,
    ) {
        // Focus mode owns the canvas zoom, so restore the user's normal view
        // before calculating a persistent 1:1 tile size.
        self.exit_focus();

        let Some((screen_rect, crop_uv, is_pinned)) = preview_manager.get(id).map(|preview| {
            (
                self.preview_screen_rect(preview, canvas_rect),
                preview.crop_uv,
                preview.viewport_pin.is_some(),
            )
        }) else {
            return;
        };
        let aligned_min = pixel_aligned_rect(
            Rect::from_min_size(screen_rect.min, Vec2::ZERO),
            pixels_per_point,
        )
        .min;
        let size = native_capture_canvas_size(
            source_size,
            crop_uv,
            pixels_per_point,
            if is_pinned { 1.0 } else { self.zoom },
        );

        if let Some(preview) = preview_manager.get_mut(id) {
            if is_pinned {
                preview.viewport_pin = Some(ViewportPin::from_rect(
                    Rect::from_min_size(aligned_min, size),
                    canvas_rect,
                ));
            } else {
                let position = self.screen_to_canvas(aligned_min, canvas_rect);
                preview.position = position;
                preview.size = size;
            }
        }
    }

    /// Get the visible canvas area
    pub fn get_viewport(&self, screen_rect: Rect) -> Rect {
        let min = self.screen_to_canvas(screen_rect.min, screen_rect);
        let max = self.screen_to_canvas(screen_rect.max, screen_rect);
        Rect::from_min_max(min, max)
    }

    /// Check if mouse is over a resize handle, returns (preview_id, handle)
    fn get_handle_at(
        &self,
        screen_pos: Pos2,
        canvas_rect: Rect,
        preview_manager: &PreviewManager,
    ) -> Option<(PreviewId, ResizeHandle)> {
        for id in &self.selection {
            if let Some(preview) = preview_manager.get(*id) {
                if preview.left_click_disabled {
                    continue;
                }
                let screen_rect = self.preview_screen_rect(preview, canvas_rect);

                let handles = [
                    (screen_rect.left_top(), ResizeHandle::TopLeft),
                    (screen_rect.center_top(), ResizeHandle::Top),
                    (screen_rect.right_top(), ResizeHandle::TopRight),
                    (screen_rect.left_center(), ResizeHandle::Left),
                    (screen_rect.right_center(), ResizeHandle::Right),
                    (screen_rect.left_bottom(), ResizeHandle::BottomLeft),
                    (screen_rect.center_bottom(), ResizeHandle::Bottom),
                    (screen_rect.right_bottom(), ResizeHandle::BottomRight),
                ];

                for (handle_pos, handle_type) in handles {
                    let handle_rect =
                        Rect::from_center_size(handle_pos, Vec2::splat(RESIZE_HANDLE_HIT_SIZE));
                    if handle_rect.contains(screen_pos) {
                        return Some((*id, handle_type));
                    }
                }
            }
        }
        None
    }

    fn preview_at_screen(
        &self,
        screen_pos: Pos2,
        canvas_rect: Rect,
        preview_manager: &PreviewManager,
    ) -> Option<PreviewId> {
        preview_manager
            .all()
            .filter(|preview| {
                preview.removing.is_none()
                    && self
                        .preview_screen_rect(preview, canvas_rect)
                        .contains(screen_pos)
            })
            .max_by_key(|preview| (preview.viewport_pin.is_some(), preview.z_order))
            .map(|preview| preview.id)
    }

    fn left_click_preview_at_screen(
        &self,
        screen_pos: Pos2,
        canvas_rect: Rect,
        preview_manager: &PreviewManager,
    ) -> Option<PreviewId> {
        preview_manager
            .all()
            .filter(|preview| {
                preview.removing.is_none()
                    && !preview.left_click_disabled
                    && self
                        .preview_screen_rect(preview, canvas_rect)
                        .contains(screen_pos)
            })
            .max_by_key(|preview| (preview.viewport_pin.is_some(), preview.z_order))
            .map(|preview| preview.id)
    }

    fn handle_pinned_pointer_drag(
        &mut self,
        id: PreviewId,
        pin: ViewportPin,
        screen_rect: Rect,
        frame: CanvasFrameScope<'_>,
        preview_manager: &mut PreviewManager,
    ) -> bool {
        let CanvasFrameScope {
            canvas_rect,
            input,
            show_overlays,
        } = frame;
        if input.pointer_blocked
            || preview_manager
                .get(id)
                .is_some_and(|preview| preview.left_click_disabled)
        {
            return false;
        }
        if let Some(drag) = self.pinned_pointer_drag.filter(|drag| drag.id == id) {
            if let Some(pointer) = input.hover_pos {
                let moved_rect = drag
                    .start_pin
                    .rect(canvas_rect)
                    .translate(pointer - drag.start_pointer);
                if let Some(preview) = preview_manager.get_mut(id) {
                    preview.viewport_pin = Some(ViewportPin::from_rect(moved_rect, canvas_rect));
                }
            }
            if input.primary_released || !input.primary_down {
                self.pinned_pointer_drag = None;
            }
            return true;
        }

        let Some(pointer) = input.interact_pos.filter(|pointer| {
            input.primary_pressed
                && !input.alt
                && !input.middle_down
                && screen_rect.contains(*pointer)
        }) else {
            return false;
        };
        if self.left_click_preview_at_screen(pointer, canvas_rect, preview_manager) != Some(id) {
            return false;
        }
        let over_resize_handle = show_overlays
            && self
                .get_handle_at(pointer, canvas_rect, preview_manager)
                .is_some();
        if over_resize_handle {
            return false;
        }

        self.selection = vec![id];
        self.pinned_pointer_drag = Some(PinnedPointerDrag {
            id,
            start_pointer: pointer,
            start_pin: pin,
        });
        true
    }

    /// Main UI rendering for the canvas
    #[cfg(test)]
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        preview_manager: &mut PreviewManager,
        capture_coordinator: &mut CaptureCoordinator,
        ctx: &egui::Context,
        show_overlays: bool,
    ) {
        self.ui_with_canvas_views_menu(
            ui,
            preview_manager,
            capture_coordinator,
            ctx,
            show_overlays,
            |_| false,
        );
    }

    pub fn ui_with_canvas_views_menu(
        &mut self,
        ui: &mut egui::Ui,
        preview_manager: &mut PreviewManager,
        capture_coordinator: &mut CaptureCoordinator,
        ctx: &egui::Context,
        show_overlays: bool,
        mut canvas_views_menu: impl FnMut(&mut egui::Ui) -> bool,
    ) {
        let canvas_rect = ui.available_rect_before_wrap();
        self.last_screen_rect = Some(canvas_rect);

        let configured_keyboard = self.keyboard_input.take();
        let (delete_pressed, select_all, escape_pressed) = match configured_keyboard {
            Some(input) => (
                input.delete_selected,
                input.select_all,
                input.exit_tile_focus,
            ),
            None => ui.input(|input| {
                (
                    input.key_pressed(egui::Key::Delete),
                    input.modifiers.ctrl && input.key_pressed(egui::Key::A),
                    input.key_pressed(egui::Key::Escape),
                )
            }),
        };

        // Snapshot the input fields we need once, instead of cloning the
        // whole InputState in every interaction pass.
        let mut input = ui.input(|i| FrameInput {
            hover_pos: i.pointer.hover_pos(),
            interact_pos: i.pointer.interact_pos(),
            pointer_delta: i.pointer.delta(),
            scroll_y: i.raw_scroll_delta.y,
            alt: i.modifiers.alt,
            ctrl: i.modifiers.ctrl,
            middle_down: i.pointer.middle_down(),
            primary_down: i.pointer.primary_down(),
            primary_pressed: i.pointer.primary_pressed(),
            primary_released: i.pointer.primary_released(),
            pointer_blocked: false,
            time: i.time,
            delete_pressed,
            select_all,
            escape_pressed,
        });
        // Canvas gestures intentionally read raw pointer input so zoom and pan
        // also work over tiles. Do not let that bypass a menu, popup, or other
        // floating egui area drawn above the canvas.
        input.pointer_blocked = input.hover_pos.is_some_and(|pointer| {
            ui.ctx()
                .layer_id_at(pointer)
                .is_some_and(|layer| layer != ui.layer_id())
        });

        if input.escape_pressed && !self.exit_focus() {
            self.cancel_camera_transition();
        }
        if !show_overlays {
            self.drag_state = None;
            self.marquee = None;
        }
        if self.is_focusing_tile()
            && matches!(
                self.drag_state,
                Some(DragState::Resizing { .. } | DragState::Cropping { .. })
            )
        {
            self.drag_state = None;
        }

        let current_time = input.time;

        // Update all animations
        self.animation.update();
        self.update_camera_transition(canvas_rect, current_time);

        // Apply momentum to pan (smooth inertia scrolling)
        let handle_drag_active = matches!(
            self.drag_state,
            Some(DragState::Resizing { .. } | DragState::Cropping { .. })
        );
        if self.animation.camera_transition.is_none()
            && self.animation.momentum_active
            && !handle_drag_active
            && !input.middle_down
            && !(input.alt && input.primary_down)
        {
            let momentum_delta = self.animation.get_momentum_delta();
            self.pan += momentum_delta / self.zoom;
        }

        // Keep the focused tile fitted after camera movement.
        self.refit_focus(preview_manager, canvas_rect);

        // Reap any previews whose fade/shrink-out animation has finished,
        // retaining the latest batch independently of the toast's visibility.
        let finished_removals = preview_manager.finalize_removals();
        if !finished_removals.is_empty() {
            self.prune_preview_interactions(preview_manager);
        }
        if let Some(info) = finished_removals.into_iter().last() {
            self.last_removed = Some((Instant::now(), info));
        }
        // A shortcut pressed during the removal animation must restore that
        // batch once source cleanup completes, rather than an older deletion.
        if self.removed_undo_requested
            && !preview_manager
                .all()
                .any(|preview| preview.removing.is_some())
        {
            self.removed_undo_requested = false;
            self.queue_removed_undo();
        }

        // CRITICAL: Allocate background interaction FIRST
        // In egui, later interactions take priority over earlier ones.
        // By allocating the canvas background first, preview interactions
        // (allocated later) will take priority and receive clicks/drags.
        let bg_response = ui.interact(
            canvas_rect,
            ui.id().with("canvas_bg"),
            Sense::click_and_drag(),
        );

        // Get the painter for drawing
        let painter = ui.painter_at(canvas_rect);

        // Draw background - Minimal Void theme (#0d0d0d)
        painter.rect_filled(canvas_rect, 0.0, Color32::from_rgb(13, 13, 13));

        if self.focus.is_none() {
            if let Some(wallpaper) = self.wallpaper.as_mut() {
                wallpaper.paint(&painter, canvas_rect, ctx);
            }
        }

        // Draw grid
        if show_overlays && self.show_grid {
            self.draw_grid(&painter, canvas_rect);
        }

        // Empty-canvas hint (only relevant before anything has been added)
        if show_overlays && preview_manager.count() == 0 {
            self.draw_empty_state(&painter, canvas_rect);
        }

        self.handle_marquee_input(canvas_rect, preview_manager, &input, show_overlays);

        let frame_scope = CanvasFrameScope {
            canvas_rect,
            input: &input,
            show_overlays,
        };

        // Draw previews and handle their interactions (AFTER bg allocation)
        self.draw_and_interact_previews(ui, frame_scope, preview_manager, ctx, capture_coordinator);

        // Draw selection rectangles and interactive resize handles
        // Handles are allocated AFTER previews so they have higher interaction priority
        if show_overlays && self.marquee.is_none() && !self.is_focusing_tile() {
            self.draw_and_interact_selection(ui, canvas_rect, preview_manager, &input);
        }

        if show_overlays {
            self.draw_marquee(&painter, canvas_rect);
        }

        // Minimal Void: Floating status indicator (bottom-right corner)
        if show_overlays && !self.is_focusing_tile() {
            self.draw_floating_status(&painter, canvas_rect, preview_manager.count());
        }
        if show_overlays {
            self.draw_and_interact_undo_toast(ui, canvas_rect);
        }

        // Handle canvas-level input using the pre-allocated bg_response
        self.handle_canvas_input_with_response(
            ui,
            frame_scope,
            preview_manager,
            capture_coordinator,
            bg_response,
            &mut canvas_views_menu,
        );

        // Apply pending FPS changes
        self.apply_pending_fps_changes(preview_manager, capture_coordinator);
        self.apply_pending_stream_audio_toggles(preview_manager);
        // Viewport culling may recreate a hibernated capture. Run it before
        // target synchronization so the new session receives its downsampled
        // output size in the same UI frame.
        self.update_viewport_culling(canvas_rect, preview_manager, capture_coordinator);
        self.sync_window_capture_targets(ctx, preview_manager, capture_coordinator);

        // Request repaint if animations are active
        if self.animation.is_animating() {
            ctx.request_repaint();
        }
    }

    fn sync_window_capture_targets(
        &self,
        ctx: &egui::Context,
        preview_manager: &PreviewManager,
        capture_coordinator: &mut CaptureCoordinator,
    ) {
        let pixels_per_point = ctx.pixels_per_point();
        let lod = capture_lod_factor(self.zoom);
        for preview in preview_manager.all() {
            if !preview.is_live_capture() || preview.manually_frozen {
                continue;
            }
            let display_size = live_capture_display_size(preview, lod);
            let (width, height) = window_capture_target(
                display_size.x,
                display_size.y,
                pixels_per_point,
                preview.crop_uv,
            );
            capture_coordinator.set_target_size(preview.id, width, height);
        }
    }

    /// Pause off-screen captures immediately, then fully release Windows
    /// capture sessions that remain far from the viewport. Process-loopback
    /// audio is owned by the app layer and intentionally stays independent.
    fn update_viewport_culling(
        &self,
        canvas_rect: Rect,
        preview_manager: &mut PreviewManager,
        capture_coordinator: &mut CaptureCoordinator,
    ) {
        let viewport = self.get_viewport(canvas_rect);
        let hibernation_margin = CAPTURE_HIBERNATE_MARGIN_POINTS / self.zoom.max(0.01);
        let nearby_viewport = viewport.expand(hibernation_margin);
        let now = Instant::now();

        // Check each preview for visibility
        for preview in preview_manager.all_mut() {
            let id = preview.id;
            let preview_rect = preview.rect();
            let is_visible = preview.viewport_pin.is_some() || viewport.intersects(preview_rect);
            let is_nearby =
                preview.viewport_pin.is_some() || nearby_viewport.intersects(preview_rect);

            if preview.manually_frozen {
                preview.capture_offscreen_since = None;
                continue;
            }

            if is_visible {
                preview.capture_offscreen_since = None;
                if preview.capture_hibernated {
                    // The app/window may have closed while capture was asleep.
                    // Let the app revalidate identity through the approved list.
                    preview.mark_window_inactive();
                } else if preview.capture_paused {
                    capture_coordinator.resume_capture(id);
                    preview.capture_paused = false;
                    #[cfg(debug_assertions)]
                    println!(
                        "Viewport culling: Resumed capture for '{}'",
                        privacy::redact_title(&preview.title)
                    );
                }
                continue;
            }

            if !preview.capture_paused {
                capture_coordinator.pause_capture(id);
                preview.capture_paused = true;
                #[cfg(debug_assertions)]
                println!(
                    "Viewport culling: Paused capture for '{}'",
                    privacy::redact_title(&preview.title)
                );
            }

            // Browser and Spout sources have different audio/render ownership.
            // Start with ordinary HWND captures, whose audio is independent.
            if !preview.is_window_capture() || preview.capture_hibernated {
                preview.capture_offscreen_since = None;
                continue;
            }

            if is_nearby {
                preview.capture_offscreen_since = None;
                continue;
            }

            let offscreen_since = preview.capture_offscreen_since.get_or_insert(now);
            if now.duration_since(*offscreen_since) >= CAPTURE_HIBERNATE_AFTER {
                capture_coordinator.stop_capture(id);
                preview.discard_pending_capture_frame();
                preview.capture_hibernated = true;
                preview.capture_offscreen_since = None;
                #[cfg(debug_assertions)]
                println!(
                    "Viewport hibernation: Released capture for '{}'",
                    privacy::redact_title(&preview.title)
                );
            }
        }
    }

    /// Apply any pending FPS changes
    fn apply_pending_fps_changes(
        &mut self,
        preview_manager: &mut PreviewManager,
        capture_coordinator: &mut CaptureCoordinator,
    ) {
        for change in self.pending_fps_changes.drain(..) {
            if let Some(preview) = preview_manager.get_mut(change.preview_id) {
                let old_fps = preview.target_fps;
                preview.set_fps_preset(change.new_fps);

                // The capture thread reads the target live; no restart (and
                // no black flash) needed.
                if preview.target_fps != old_fps {
                    capture_coordinator.set_target_fps(change.preview_id, preview.target_fps);
                }
            }
        }
    }

    fn apply_pending_stream_audio_toggles(&mut self, preview_manager: &mut PreviewManager) {
        for id in self.pending_stream_audio_toggles.drain(..) {
            if let Some(preview) = preview_manager.get_mut(id) {
                if preview.is_window_capture() {
                    preview.stream_audio = !preview.stream_audio;
                }
            }
        }
    }

    fn paint_stream_audio_badge(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        id: PreviewId,
        screen_rect: Rect,
        enabled: bool,
        beside_fps: bool,
    ) {
        let badge_rect = stream_audio_badge_rect(screen_rect, enabled, beside_fps);
        let response = ui
            .interact(
                badge_rect,
                ui.id().with(("stream_audio", id.0)),
                Sense::click(),
            )
            .on_hover_text(stream_audio_tooltip(enabled, self.stream_monitor_ready));
        let hovered = response.hovered();
        let fill = if enabled {
            Color32::from_rgba_unmultiplied(18, 42, 72, if hovered { 230 } else { 200 })
        } else {
            Color32::from_rgba_unmultiplied(0, 0, 0, if hovered { 210 } else { 180 })
        };
        painter.rect_filled(badge_rect, 10.0, fill);
        painter.text(
            badge_rect.center(),
            egui::Align2::CENTER_CENTER,
            if enabled {
                "Stream Audio: On"
            } else {
                "Stream Audio"
            },
            egui::FontId::proportional(10.0),
            if enabled {
                Color32::from_rgb(140, 200, 255)
            } else {
                Color32::from_rgb(150, 150, 150)
            },
        );
        if response.clicked() {
            self.pending_stream_audio_toggles.push(id);
        }
    }

    fn draw_marquee(&self, painter: &egui::Painter, canvas_rect: Rect) {
        let Some(marquee) = self.marquee.as_ref().filter(|marquee| marquee.is_drag()) else {
            return;
        };
        let rect = marquee.rect().intersect(canvas_rect);
        painter.rect_filled(rect, 2.0, Color32::from_rgba_unmultiplied(74, 158, 255, 34));
        painter.rect_stroke(rect, 2.0, Stroke::new(1.0, Color32::from_rgb(74, 158, 255)));
    }

    fn update_marquee_selection(
        &mut self,
        canvas_rect: Rect,
        preview_manager: &PreviewManager,
        pointer: Pos2,
    ) {
        let Some(marquee) = self.marquee.as_mut() else {
            return;
        };
        marquee.current = pointer;
        if !marquee.is_drag() {
            return;
        }

        let selection_rect = marquee.rect().intersect(canvas_rect);
        let mut selected = marquee.base_selection.clone();
        for preview in preview_manager
            .all()
            .filter(|preview| preview.removing.is_none() && !preview.left_click_disabled)
        {
            let screen_rect = self.preview_screen_rect(preview, canvas_rect);
            if selection_rect.intersects(screen_rect) && !selected.contains(&preview.id) {
                selected.push(preview.id);
            }
        }
        self.selection = selected;
    }

    fn handle_marquee_input(
        &mut self,
        canvas_rect: Rect,
        preview_manager: &PreviewManager,
        input: &FrameInput,
        show_overlays: bool,
    ) {
        // Left-dragging from empty canvas draws a marquee. Tile drags still
        // move tiles, while Alt+left and middle-button drags remain panning.
        if show_overlays
            && !input.pointer_blocked
            && input.primary_pressed
            && !input.alt
            && !input.middle_down
            && self.drag_state.is_none()
        {
            if let Some(pointer) = input.interact_pos.filter(|pos| canvas_rect.contains(*pos)) {
                let over_resize_handle = self
                    .get_handle_at(pointer, canvas_rect, preview_manager)
                    .is_some();
                if !over_resize_handle
                    && self
                        .left_click_preview_at_screen(pointer, canvas_rect, preview_manager)
                        .is_none()
                {
                    self.marquee = Some(MarqueeSelection {
                        start: pointer,
                        current: pointer,
                        base_selection: if input.ctrl {
                            self.selection.clone()
                        } else {
                            Vec::new()
                        },
                    });
                }
            }
        }
        if self.marquee.is_some() {
            if let Some(pointer) = input.hover_pos.or(input.interact_pos) {
                self.update_marquee_selection(canvas_rect, preview_manager, pointer);
            }
            if input.primary_released {
                self.marquee = None;
            }
        }
    }

    fn queue_activity_for_selection(&mut self, ids: &[PreviewId], action: TileActivityAction) {
        self.pending_tile_activity_actions
            .extend(ids.iter().copied().map(|id| (id, action)));
    }

    /// Handle canvas-level input (background clicks, pan, zoom)
    fn handle_canvas_input_with_response(
        &mut self,
        ui: &mut egui::Ui,
        frame: CanvasFrameScope<'_>,
        preview_manager: &mut PreviewManager,
        capture_coordinator: &mut CaptureCoordinator,
        bg_response: egui::Response,
        canvas_views_menu: &mut impl FnMut(&mut egui::Ui) -> bool,
    ) {
        let CanvasFrameScope {
            canvas_rect,
            input,
            show_overlays,
        } = frame;
        // Use the pre-allocated background response

        // Update cursor based on drag state or handle hover
        if show_overlays && !self.is_focusing_tile() {
            if let Some(mouse_pos) = input.hover_pos {
                if !input.pointer_blocked && canvas_rect.contains(mouse_pos) {
                    if let Some((_, handle)) =
                        self.get_handle_at(mouse_pos, canvas_rect, preview_manager)
                    {
                        ui.ctx().set_cursor_icon(handle.cursor());
                    }
                }
            }
        }

        let handle_drag_active = matches!(
            self.drag_state,
            Some(DragState::Resizing { .. } | DragState::Cropping { .. })
        );

        // Zoom with scroll wheel - works anywhere on canvas, even over previews
        // We check canvas_rect.contains() instead of bg_response.hovered() because
        // bg_response.hovered() returns false when the mouse is over a preview widget
        if !handle_drag_active && !input.pointer_blocked {
            if let Some(mouse_pos) = input.hover_pos {
                if canvas_rect.contains(mouse_pos) {
                    let scroll_delta = input.scroll_y;
                    if scroll_delta != 0.0 {
                        let canvas_pos = self.screen_to_canvas(mouse_pos, canvas_rect);
                        let playlist_id = preview_manager.get_preview_at(canvas_pos).filter(|id| {
                            preview_manager.get(*id).is_some_and(Preview::is_playlist)
                        });
                        if let Some(id) = playlist_id {
                            if let Some(playlist) = preview_manager
                                .get_mut(id)
                                .and_then(|preview| preview.folder_playlist.as_mut())
                            {
                                playlist.scroll_offset = (playlist.scroll_offset
                                    - scroll_delta / playlist_zoom(self.zoom))
                                .max(0.0);
                            }
                        } else {
                            self.cancel_camera_transition();
                            let zoom_factor = if scroll_delta > 0.0 { 1.1 } else { 0.9 };
                            let new_zoom =
                                (self.zoom * zoom_factor).clamp(self.zoom_min, self.zoom_max);
                            self.zoom = new_zoom;
                            let new_canvas_pos = self.screen_to_canvas(mouse_pos, canvas_rect);
                            self.pan += new_canvas_pos.to_vec2() - canvas_pos.to_vec2();
                        }
                    }
                }
            }
        }

        // Pan with middle mouse button or Alt+Left drag
        // Works anywhere on canvas, even over previews (similar to zoom)
        let is_panning = !handle_drag_active
            && !input.pointer_blocked
            && (input.middle_down || (input.alt && input.primary_down))
            && canvas_rect.contains(input.hover_pos.unwrap_or_default());

        if handle_drag_active {
            self.canvas_panning = false;
            self.pan_drag_tracker.clear();
            self.animation.momentum_active = false;
            self.animation.momentum_velocity = Vec2::ZERO;
        } else if is_panning {
            // Start panning
            if !self.canvas_panning {
                self.canvas_panning = true;
                self.pan_drag_tracker.clear();
                self.cancel_camera_transition();
                // Stop any existing momentum
                self.animation.momentum_active = false;
                self.animation.momentum_velocity = Vec2::ZERO;
            }

            // Track velocity for momentum
            if let Some(mouse_pos) = input.hover_pos {
                self.pan_drag_tracker.record(mouse_pos, input.time);
            }

            // Use pointer delta directly instead of bg_response.drag_delta()
            // because bg_response.dragged() returns false when over a preview
            let delta = input.pointer_delta;
            if delta != Vec2::ZERO {
                self.pan += delta / self.zoom;
            }
        } else if self.canvas_panning {
            // Just released pan - apply momentum
            self.canvas_panning = false;
            let velocity = self.pan_drag_tracker.get_velocity();
            self.animation.start_momentum(velocity);
        }

        // Click on empty space to deselect
        if bg_response.clicked() && !input.ctrl {
            if let Some(mouse_pos) = input.interact_pos {
                if self
                    .left_click_preview_at_screen(mouse_pos, canvas_rect, preview_manager)
                    .is_none()
                {
                    self.selection.clear();
                }
            }
        }

        // Canvas context menu (right-click on empty space). Disabled tiles do
        // not own primary interaction, so the background response may also
        // report their secondary click; exclude every visible tile explicitly.
        let background_secondary_target = ui
            .input(|input| input.pointer.button_clicked(egui::PointerButton::Secondary))
            && !input.pointer_blocked
            && input.interact_pos.is_some_and(|pointer| {
                self.preview_at_screen(pointer, canvas_rect, preview_manager)
                    .is_none()
            });
        let mut background_context_response = bg_response.clone();
        if background_secondary_target {
            background_context_response.sense = Sense::click();
            background_context_response.contains_pointer = true;
            background_context_response.hovered = true;
            background_context_response.clicked = true;
            background_context_response.interact_pointer_pos = input.interact_pos;
            self.last_secondary_click = input.interact_pos;
        }

        let background_context_menu_active =
            background_context_response.context_menu_opened() || background_secondary_target;
        show_canvas_context_menu_if(
            &background_context_response,
            background_context_menu_active,
            |ui| {
                if ui.button("Add Window...").clicked() {
                    if let Some(screen_pos) = self.last_secondary_click {
                        let canvas_pos = self.screen_to_canvas(screen_pos, canvas_rect);
                        self.pending_quick_add = Some((canvas_pos, screen_pos));
                    }
                    ui.close_menu();
                }
                if ui.button("Add Browser...").clicked() {
                    if let Some(screen_pos) = self.last_secondary_click {
                        self.pending_browser_add =
                            Some(self.screen_to_canvas(screen_pos, canvas_rect));
                    }
                    ui.close_menu();
                }
                if ui.button("Add Image...").clicked() {
                    if let Some(screen_pos) = self.last_secondary_click {
                        self.pending_media_add =
                            Some(self.screen_to_canvas(screen_pos, canvas_rect));
                    }
                    ui.close_menu();
                }
                if ui.button("Add Video...").clicked() {
                    if let Some(screen_pos) = self.last_secondary_click {
                        self.pending_video_add =
                            Some(self.screen_to_canvas(screen_pos, canvas_rect));
                    }
                    ui.close_menu();
                }
                if ui.button("Add Stream...").clicked() {
                    if let Some(screen_pos) = self.last_secondary_click {
                        self.pending_stream_add =
                            Some(self.screen_to_canvas(screen_pos, canvas_rect));
                    }
                    ui.close_menu();
                }
                ui.separator();
                saved_viewpoints_submenu(ui, |ui| canvas_views_menu(ui));
                if ui.button("Reset View").clicked() {
                    self.reset();
                    ui.close_menu();
                }
                ui.separator();
                ui.checkbox(&mut self.show_grid, "Show Grid");
                if ui.button("Set Wallpaper...").clicked() {
                    self.pending_wallpaper_pick = true;
                    ui.close_menu();
                }
                if self.wallpaper.is_some() && ui.button("Clear Wallpaper").clicked() {
                    self.pending_wallpaper_clear = true;
                    ui.close_menu();
                }
                ui.separator();
                if !self.selection.is_empty() {
                    let selected = self.selection.clone();
                    let any_live = selected.iter().any(|id| {
                        preview_manager
                            .get(*id)
                            .is_some_and(|preview| !preview.manually_frozen)
                    });
                    let any_frozen = selected.iter().any(|id| {
                        preview_manager
                            .get(*id)
                            .is_some_and(|preview| preview.manually_frozen)
                    });
                    if any_live && ui.button("Freeze Selected").clicked() {
                        self.queue_activity_for_selection(&selected, TileActivityAction::Freeze);
                        ui.close_menu();
                    }
                    if any_frozen && ui.button("Resume Selected").clicked() {
                        self.queue_activity_for_selection(&selected, TileActivityAction::Resume);
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button("Remove Selected").clicked() {
                        for id in &self.selection {
                            capture_coordinator.stop_capture(*id);
                        }
                        preview_manager.start_removal_batch(&self.selection);
                        self.selection.clear();
                        ui.close_menu();
                    }
                }
            },
        );

        // Keyboard shortcuts are app-wide while Pluriview owns focus. They do
        // not depend on pointer position; the app suppresses them while a text
        // field or shortcut recorder owns keyboard input.
        if input.delete_pressed {
            for id in &self.selection {
                capture_coordinator.stop_capture(*id);
            }
            preview_manager.start_removal_batch(&self.selection);
            self.selection.clear();
        }

        if input.select_all {
            self.selection = preview_manager
                .all()
                .filter(|preview| !preview.left_click_disabled)
                .map(|preview| preview.id)
                .collect();
        }
    }

    /// Draw previews and handle their individual interactions
    fn draw_and_interact_previews(
        &mut self,
        ui: &mut egui::Ui,
        frame: CanvasFrameScope<'_>,
        preview_manager: &mut PreviewManager,
        ctx: &egui::Context,
        capture_coordinator: &mut CaptureCoordinator,
    ) {
        let CanvasFrameScope {
            canvas_rect,
            input,
            show_overlays,
        } = frame;
        let viewport = self.get_viewport(canvas_rect);

        // Reuse one allocation for the visible, z-sorted interaction snapshot.
        // The snapshot releases the manager borrow before tiles mutate it.
        let mut preview_info = std::mem::take(&mut self.tile_scratch);
        let mut visible_count = 0;
        for preview in preview_manager
            .all()
            .filter(|preview| preview.viewport_pin.is_some() || preview.rect().intersects(viewport))
        {
            if let Some(info) = preview_info.get_mut(visible_count) {
                info.update_from(preview);
            } else {
                preview_info.push(TileInfo::from_preview(preview));
            }
            visible_count += 1;
        }
        preview_info.truncate(visible_count);
        // Pinned tiles are a viewport overlay and always paint above
        // ordinary canvas content while preserving their relative z-order.
        preview_info.sort_by_key(|info| (info.viewport_pin.is_some(), info.z_order));

        let mut any_spawn_or_remove_animating = false;
        let mut window_capture_connecting = false;
        let mut transparent_browser_waiting = false;

        for info in &preview_info {
            let id = info.id;
            let rect = info.rect;
            let title = &info.title;
            let target_fps = info.target_fps;
            let current_preset = info.fps_preset;
            let has_crop = info.has_crop;
            let crop_uv = info.crop_uv;
            let is_removing = info.is_removing;
            let spawn_t = info.spawn_t;
            let remove_t = info.remove_t;
            let is_browser = info.is_browser;
            let is_media = info.is_media;
            let is_video = info.is_video;
            let is_playlist = info.is_playlist;
            let is_window_capture = info.is_window_capture;
            let is_inactive_window = info.is_inactive_window;
            let window_exe = info.window_exe.as_deref();
            let is_spout_capture = info.is_spout_capture;
            let viewport_pin = info.viewport_pin;
            let lock_aspect_ratio = info.lock_aspect_ratio;
            let left_click_disabled = info.left_click_disabled;
            let muted = if is_video {
                info.video_playback.muted
            } else {
                info.muted
            };
            let stream_audio = info.stream_audio;
            let capture_failed = info.capture_failed;
            let browser_status = &info.browser_status;
            let browser_waiting_for_content = info.browser_waiting_for_content;
            let video_status = &info.video_status;
            let video_playback = &info.video_playback;
            let supports_seek_preview = info.supports_seek_preview;
            let manually_frozen = info.manually_frozen;
            let frame_size = info.frame_size;
            let source_frame_size = info.source_frame_size;
            let screen_rect = viewport_pin
                .map(|pin| pin.rect(canvas_rect))
                .unwrap_or_else(|| self.canvas_rect_to_screen(rect, canvas_rect));
            let screen_rect = if is_window_capture {
                pixel_aligned_rect(screen_rect, ctx.pixels_per_point())
            } else {
                screen_rect
            };

            if !canvas_rect.intersects(screen_rect) {
                continue;
            }

            // Spawn-in / fade-out: ease alpha and a subtle scale toward center.
            let (alpha, scale) = if is_removing {
                any_spawn_or_remove_animating |= remove_t < 1.0;
                (1.0 - remove_t, 1.0 - remove_t * 0.12)
            } else if spawn_t < 1.0 {
                any_spawn_or_remove_animating = true;
                (spawn_t, 0.85 + spawn_t * 0.15)
            } else {
                (1.0, 1.0)
            };
            let anim_rect = if (scale - 1.0).abs() > f32::EPSILON {
                Rect::from_center_size(screen_rect.center(), screen_rect.size() * scale)
            } else {
                screen_rect
            };
            let anim_rect = if is_window_capture {
                pixel_aligned_rect(anim_rect, ctx.pixels_per_point())
            } else {
                anim_rect
            };
            let alpha_u8 = (alpha.clamp(0.0, 1.0) * 255.0) as u8;

            let painter = ui.painter_at(canvas_rect);

            if is_removing {
                // Fading out: paint the last frame only, no interaction.
                if let Some(preview) = preview_manager.get_mut(id) {
                    let uv_rect = preview.get_uv_rect();
                    if let Some(texture) = preview.get_texture(ctx) {
                        painter.image(
                            texture.id(),
                            anim_rect,
                            uv_rect,
                            Color32::from_white_alpha(alpha_u8),
                        );
                    }
                }
                continue;
            }

            let secondary_clicked_raw =
                ui.input(|input| input.pointer.button_clicked(egui::PointerButton::Secondary));
            let secondary_target = !input.pointer_blocked
                && secondary_clicked_raw
                && input
                    .interact_pos
                    .or(input.hover_pos)
                    .is_some_and(|pointer| {
                        self.preview_at_screen(pointer, canvas_rect, preview_manager) == Some(id)
                    });

            // Create interactive area for primary input. A disabled tile uses
            // hover-only sense so tiles underneath can own clicks and drags.
            let preview_response = ui.interact(
                screen_rect,
                ui.id().with(("preview", id.0)),
                if left_click_disabled {
                    Sense::hover()
                } else {
                    Sense::click_and_drag()
                },
            );
            // Context menus are button-agnostic in egui, so registering click
            // sense on a disabled tile would also swallow primary input. Route
            // the raw secondary click to the visually topmost tile and mark a
            // cloned response as clicked without changing widget ownership.
            let mut context_response = preview_response.clone();
            if secondary_target {
                context_response.sense = Sense::click();
                context_response.contains_pointer = true;
                context_response.hovered = true;
                context_response.clicked = true;
                context_response.interact_pointer_pos = input.interact_pos.or(input.hover_pos);
            }

            let pinned_pointer_active = viewport_pin.is_some_and(|pin| {
                self.handle_pinned_pointer_drag(id, pin, screen_rect, frame, preview_manager)
            });

            // Minimal Void: No background fill - content fills entire area
            // Draw preview content (full rect, no title bar offset)
            #[cfg(windows)]
            let has_direct_video = if matches!(video_status, VideoTileStatus::Failed(_)) {
                false
            } else {
                preview_manager
                    .get(id)
                    .and_then(|preview| preview.video_renderer.clone())
                    .map(|renderer| {
                        painter.add(egui::PaintCallback {
                            rect: anim_rect,
                            callback: std::sync::Arc::new(eframe::egui_glow::CallbackFn::new(
                                move |info, painter| {
                                    renderer.paint_cropped(info, painter.gl(), crop_uv);
                                },
                            )),
                        });
                        true
                    })
                    .unwrap_or(false)
            };
            #[cfg(not(windows))]
            let has_direct_video = false;

            let has_playlist_content = if is_playlist {
                self.paint_folder_playlist(ui, &painter, anim_rect, preview_manager, id, alpha_u8);
                true
            } else {
                false
            };

            // A transparent WebView frame is valid for overlay pages, but the
            // first completely transparent frames look like the tile vanished.
            // Keep the loading card underneath until capture sees content.
            let browser_loading_placeholder =
                is_browser && browser_waiting_for_content && !manually_frozen;
            if browser_loading_placeholder {
                paint_browser_placeholder(&painter, anim_rect, browser_status, input.time as f32);
                transparent_browser_waiting |=
                    !matches!(browser_status, BrowserTileStatus::Failed(_));
            }

            let has_texture = if has_playlist_content || has_direct_video {
                true
            } else if let Some(preview) = preview_manager.get_mut(id) {
                // Get UV rect first (immutable borrow ends before get_texture)
                let uv_rect = preview.get_uv_rect();
                if let Some(texture) = preview.get_texture(ctx) {
                    // Minimal Void: content fills entire rect
                    painter.image(
                        texture.id(),
                        anim_rect,
                        uv_rect,
                        Color32::from_white_alpha(alpha_u8),
                    );
                    true
                } else {
                    false
                }
            } else {
                false
            };

            if !has_texture {
                if is_media && capture_failed {
                    paint_missing_media_placeholder(&painter, anim_rect);
                } else if manually_frozen {
                    painter.rect_filled(anim_rect, 8.0, Color32::from_rgb(22, 26, 32));
                    painter.text(
                        anim_rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "Frozen",
                        egui::FontId::proportional(12.0),
                        Color32::from_rgb(140, 170, 205),
                    );
                } else if is_browser && !browser_loading_placeholder {
                    paint_browser_placeholder(
                        &painter,
                        anim_rect,
                        browser_status,
                        input.time as f32,
                    );
                    any_spawn_or_remove_animating |=
                        !matches!(browser_status, BrowserTileStatus::Failed(_));
                } else if is_video {
                    any_spawn_or_remove_animating |= paint_video_placeholder(
                        &painter,
                        anim_rect,
                        video_status,
                        input.time as f32,
                    );
                } else if is_inactive_window {
                    paint_inactive_window_placeholder(&painter, anim_rect, window_exe);
                } else {
                    window_capture_connecting |= paint_window_capture_placeholder(
                        &painter,
                        anim_rect,
                        capture_failed,
                        input.time as f32,
                    );
                }
            }

            // Minimal Void: Hover-reveal controls (no permanent title bar).
            // Do not gate these child widgets on `preview_response.hovered()`:
            // once a child (such as the close button) owns the pointer, egui
            // may stop reporting the parent as hovered on the mouse-down
            // frame. That removes the child before it can receive the release
            // and makes the button appear inert. A geometry/z-order hit test
            // stays stable for the complete click while still limiting the
            // controls to the topmost tile under the pointer.
            let pointer_over_tile = !input.pointer_blocked
                && input.hover_pos.is_some_and(|pointer_pos| {
                    screen_rect.contains(pointer_pos)
                        && self.left_click_preview_at_screen(
                            pointer_pos,
                            canvas_rect,
                            preview_manager,
                        ) == Some(id)
                });
            if show_overlays && pointer_over_tile {
                // Playlist tiles already have a designed header; a second title
                // bar would cover the folder name and transport controls.
                if !is_playlist && !is_spout_capture {
                    let overlay_rect =
                        Rect::from_min_size(screen_rect.min, Vec2::new(screen_rect.width(), 40.0));
                    painter.rect_filled(
                        overlay_rect,
                        egui::Rounding {
                            nw: 8.0,
                            ne: 8.0,
                            sw: 0.0,
                            se: 0.0,
                        },
                        Color32::from_rgba_unmultiplied(0, 0, 0, 120),
                    );
                }

                // Close button (top-right)
                let close_scale = if is_playlist {
                    playlist_zoom(self.zoom)
                } else {
                    1.0
                };
                let close_btn_rect = Rect::from_min_size(
                    screen_rect.right_top()
                        + Vec2::new(
                            playlist_px(-32.0, close_scale),
                            playlist_px(8.0, close_scale),
                        ),
                    Vec2::splat(playlist_px(24.0, close_scale)),
                );
                let close_response = ui.interact(
                    close_btn_rect,
                    ui.id().with(("preview_close", id.0)),
                    Sense::click(),
                );
                let close_bg = if close_response.hovered() {
                    Color32::from_rgba_unmultiplied(255, 100, 100, 230)
                } else {
                    Color32::from_rgba_unmultiplied(255, 80, 80, 200)
                };
                painter.rect_filled(close_btn_rect, playlist_px(4.0, close_scale), close_bg);
                painter.text(
                    close_btn_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    egui_phosphor::regular::X,
                    playlist_font(13.0, close_scale),
                    Color32::WHITE,
                );
                if close_response.clicked() {
                    capture_coordinator.stop_capture(id);
                    preview_manager.start_removal(id);
                    self.selection.retain(|&x| x != id);
                }

                // Capture FPS does not apply to file-backed image tiles.
                if !is_media && !is_playlist {
                    let fps_text = format!("{}", target_fps);
                    let fps_rect = Rect::from_min_size(
                        screen_rect.right_top() + Vec2::new(-72.0, 10.0),
                        Vec2::new(36.0, 20.0),
                    );
                    painter.rect_filled(
                        fps_rect,
                        10.0,
                        Color32::from_rgba_unmultiplied(0, 0, 0, 180),
                    );
                    painter.text(
                        fps_rect.center(),
                        egui::Align2::CENTER_CENTER,
                        &fps_text,
                        egui::FontId::proportional(10.0),
                        Color32::from_rgb(150, 150, 150),
                    );
                    if is_window_capture {
                        self.paint_stream_audio_badge(
                            ui,
                            &painter,
                            id,
                            screen_rect,
                            stream_audio,
                            true,
                        );
                    }
                    if is_browser || is_window_capture || is_spout_capture {
                        if let Some((width, height)) = frame_size {
                            let left_of = if is_window_capture {
                                stream_audio_badge_rect(screen_rect, stream_audio, true)
                            } else {
                                fps_rect
                            };
                            let resolution_rect = capture_resolution_badge_rect(left_of);
                            let resolution_text = format!("{width}×{height}");
                            ui.interact(
                                resolution_rect,
                                ui.id().with(("capture_resolution", id.0)),
                                Sense::hover(),
                            )
                            .on_hover_text("Capture resolution");
                            painter.rect_filled(
                                resolution_rect,
                                10.0,
                                Color32::from_rgba_unmultiplied(0, 0, 0, 180),
                            );
                            painter.text(
                                resolution_rect.center(),
                                egui::Align2::CENTER_CENTER,
                                resolution_text,
                                egui::FontId::proportional(10.0),
                                Color32::from_rgb(150, 150, 150),
                            );
                        }
                    }
                }

                if !is_playlist {
                    // Title (truncated, on the left) - handle UTF-8 properly
                    let title_text = compact_title(title, 25);
                    let title_pos = if is_browser || is_media || is_video || is_spout_capture {
                        // Source badge marks app-owned tiles; shift the title right.
                        painter.text(
                            screen_rect.left_top() + Vec2::new(12.0, 20.0),
                            egui::Align2::LEFT_CENTER,
                            if is_browser {
                                egui_phosphor::regular::GLOBE
                            } else if is_video {
                                egui_phosphor::regular::VIDEO
                            } else if is_spout_capture {
                                egui_phosphor::regular::BROADCAST
                            } else {
                                egui_phosphor::regular::IMAGE
                            },
                            egui::FontId::proportional(12.0),
                            Color32::from_rgb(207, 161, 57),
                        );
                        screen_rect.left_top() + Vec2::new(28.0, 20.0)
                    } else {
                        screen_rect.left_top() + Vec2::new(12.0, 20.0)
                    };
                    painter.text(
                        title_pos,
                        egui::Align2::LEFT_CENTER,
                        title_text,
                        egui::FontId::proportional(11.0),
                        Color32::from_rgb(200, 200, 200),
                    );
                }

                // Browser tiles: navigation + audio controls along the bottom
                if is_browser && !manually_frozen && *browser_status == BrowserTileStatus::Ready {
                    let bottom_overlay = Rect::from_min_size(
                        screen_rect.left_bottom() + Vec2::new(0.0, -42.0),
                        Vec2::new(screen_rect.width(), 42.0),
                    );
                    painter.rect_filled(
                        bottom_overlay,
                        egui::Rounding {
                            nw: 0.0,
                            ne: 0.0,
                            sw: 8.0,
                            se: 8.0,
                        },
                        Color32::from_rgba_unmultiplied(0, 0, 0, 120),
                    );

                    let buttons: [(&str, BrowserAction, &str); 5] = [
                        (
                            egui_phosphor::regular::CARET_LEFT,
                            BrowserAction::Back,
                            "Back",
                        ),
                        (
                            egui_phosphor::regular::CARET_RIGHT,
                            BrowserAction::Forward,
                            "Forward",
                        ),
                        (
                            egui_phosphor::regular::ARROW_CLOCKWISE,
                            BrowserAction::Reload,
                            "Reload",
                        ),
                        (
                            if muted {
                                egui_phosphor::regular::SPEAKER_SLASH
                            } else {
                                egui_phosphor::regular::SPEAKER_HIGH
                            },
                            BrowserAction::ToggleMute,
                            if muted { "Unmute" } else { "Mute" },
                        ),
                        (
                            egui_phosphor::regular::ARROW_SQUARE_OUT,
                            BrowserAction::OpenExternal,
                            "Open in browser",
                        ),
                    ];
                    for (idx, (icon, action, tip)) in buttons.iter().enumerate() {
                        let btn_rect = Rect::from_min_size(
                            screen_rect.left_bottom() + Vec2::new(10.0 + idx as f32 * 30.0, -34.0),
                            Vec2::splat(26.0),
                        );
                        let resp = ui
                            .interact(
                                btn_rect,
                                ui.id().with(("browser_btn", id.0, idx)),
                                Sense::click(),
                            )
                            .on_hover_text(*tip);
                        let (background, icon_color) =
                            browser_control_colors(*action, resp.hovered(), muted);
                        if let Some(background) = background {
                            painter.rect_filled(btn_rect, 6.0, background);
                        }
                        painter.text(
                            btn_rect.center(),
                            egui::Align2::CENTER_CENTER,
                            *icon,
                            egui::FontId::proportional(14.0),
                            icon_color,
                        );
                        if resp.clicked() {
                            self.pending_browser_actions.push((id, *action));
                        }
                    }
                }

                if is_video && !manually_frozen {
                    let bottom_overlay = Rect::from_min_size(
                        screen_rect.left_bottom() + Vec2::new(0.0, -48.0),
                        Vec2::new(screen_rect.width(), 48.0),
                    );
                    painter.rect_filled(
                        bottom_overlay,
                        egui::Rounding {
                            nw: 0.0,
                            ne: 0.0,
                            sw: 8.0,
                            se: 8.0,
                        },
                        Color32::from_rgba_unmultiplied(0, 0, 0, 155),
                    );

                    let controls_enabled = video_playback.connected
                        && !matches!(video_status, VideoTileStatus::Failed(_));
                    let play_rect = Rect::from_min_size(
                        bottom_overlay.left_top() + Vec2::new(8.0, 13.0),
                        Vec2::splat(26.0),
                    );
                    let play_response = ui
                        .interact(
                            play_rect,
                            ui.id().with(("video_play", id.0)),
                            Sense::click(),
                        )
                        .on_hover_text(if video_playback.paused {
                            "Play"
                        } else {
                            "Pause"
                        });
                    if play_response.hovered() || play_response.is_pointer_button_down_on() {
                        painter.rect_filled(
                            play_rect,
                            6.0,
                            Color32::from_rgba_unmultiplied(
                                255,
                                255,
                                255,
                                if play_response.is_pointer_button_down_on() {
                                    65
                                } else {
                                    35
                                },
                            ),
                        );
                    }
                    painter.text(
                        play_rect.center(),
                        egui::Align2::CENTER_CENTER,
                        if video_playback.paused {
                            egui_phosphor::regular::PLAY
                        } else {
                            egui_phosphor::regular::PAUSE
                        },
                        egui::FontId::proportional(15.0),
                        if controls_enabled {
                            Color32::from_rgb(225, 225, 230)
                        } else {
                            Color32::from_rgb(100, 100, 106)
                        },
                    );
                    if controls_enabled && play_response.clicked() {
                        self.pending_video_actions
                            .push((id, VideoAction::SetPaused(!video_playback.paused)));
                    }

                    let reload_rect = Rect::from_min_size(
                        bottom_overlay.left_top() + Vec2::new(38.0, 13.0),
                        Vec2::splat(26.0),
                    );
                    let reload_response = ui
                        .interact(
                            reload_rect,
                            ui.id().with(("video_reload", id.0)),
                            Sense::click(),
                        )
                        .on_hover_text("Reload video");
                    if reload_response.hovered() {
                        painter.rect_filled(
                            reload_rect,
                            6.0,
                            Color32::from_rgba_unmultiplied(255, 255, 255, 35),
                        );
                    }
                    painter.text(
                        reload_rect.center(),
                        egui::Align2::CENTER_CENTER,
                        egui_phosphor::regular::ARROW_CLOCKWISE,
                        egui::FontId::proportional(15.0),
                        Color32::from_rgb(225, 225, 230),
                    );
                    if reload_response.clicked() {
                        self.pending_video_actions.push((id, VideoAction::Reload));
                    }

                    let mute_rect = Rect::from_min_size(
                        bottom_overlay.right_top() + Vec2::new(-34.0, 13.0),
                        Vec2::splat(26.0),
                    );
                    let mute_response = ui.interact(
                        mute_rect,
                        ui.id().with(("video_mute", id.0)),
                        Sense::click(),
                    );
                    painter.text(
                        mute_rect.center(),
                        egui::Align2::CENTER_CENTER,
                        if muted {
                            egui_phosphor::regular::SPEAKER_SLASH
                        } else {
                            egui_phosphor::regular::SPEAKER_HIGH
                        },
                        egui::FontId::proportional(15.0),
                        if muted {
                            Color32::from_rgb(255, 150, 100)
                        } else if controls_enabled {
                            Color32::from_rgb(225, 225, 230)
                        } else {
                            Color32::from_rgb(100, 100, 106)
                        },
                    );
                    if controls_enabled && mute_response.clicked() {
                        self.pending_video_actions
                            .push((id, VideoAction::ToggleMute));
                    }

                    let volume_popover = video_volume_popover_rect(screen_rect, mute_rect);
                    let pointer_over_volume_controls = input
                        .hover_pos
                        .is_some_and(|pointer| mute_rect.union(volume_popover).contains(pointer));
                    if mute_response.hovered()
                        || (self.video_volume_hover == Some(id) && pointer_over_volume_controls)
                    {
                        self.video_volume_hover = Some(id);
                    } else if self.video_volume_hover == Some(id) {
                        self.video_volume_hover = None;
                    }

                    if self.video_volume_hover == Some(id) {
                        painter.rect_filled(
                            volume_popover,
                            7.0,
                            Color32::from_rgba_unmultiplied(18, 18, 23, 238),
                        );
                        painter.rect_stroke(
                            volume_popover,
                            7.0,
                            Stroke::new(1.0, Color32::from_rgb(70, 70, 78)),
                        );

                        let mut volume = video_playback.volume;
                        let label_rect = Rect::from_min_max(
                            volume_popover.min + Vec2::new(4.0, 4.0),
                            Pos2::new(volume_popover.right() - 4.0, volume_popover.top() + 22.0),
                        );
                        painter.text(
                            label_rect.center(),
                            egui::Align2::CENTER_CENTER,
                            format!("{volume:.0}%"),
                            egui::FontId::monospace(10.0),
                            Color32::from_rgb(215, 215, 222),
                        );
                        let slider_rect = video_volume_slider_rect(volume_popover);
                        let volume_response = ui
                            .allocate_new_ui(
                                egui::UiBuilder::new().max_rect(slider_rect).layout(
                                    egui::Layout::centered_and_justified(egui::Direction::TopDown),
                                ),
                                |ui| {
                                    // Vertical sliders otherwise keep egui's default 100-point
                                    // track and can overflow a compact popup.
                                    ui.spacing_mut().slider_width = slider_rect.height();
                                    ui.add(
                                        egui::Slider::new(&mut volume, 0.0..=100.0)
                                            .vertical()
                                            .show_value(false)
                                            .trailing_fill(true),
                                    )
                                },
                            )
                            .inner;
                        if controls_enabled && volume_response.changed() {
                            self.pending_video_actions
                                .push((id, VideoAction::SetVolume(volume)));
                        }
                    }

                    let time_text = format!(
                        "{} / {}",
                        format_time(video_playback.time_pos),
                        format_time(video_playback.duration),
                    );
                    let (time_width, time_font_size) = video_time_style(screen_rect.width());
                    let time_rect = Rect::from_min_size(
                        Pos2::new(
                            mute_rect.left() - time_width - 4.0,
                            bottom_overlay.top() + 15.0,
                        ),
                        Vec2::new(time_width, 22.0),
                    );
                    painter.text(
                        time_rect.center(),
                        egui::Align2::CENTER_CENTER,
                        time_text,
                        egui::FontId::monospace(time_font_size),
                        Color32::from_rgb(190, 190, 198),
                    );

                    let progress_rect = Rect::from_min_max(
                        Pos2::new(reload_rect.right() + 8.0, bottom_overlay.top() + 20.0),
                        Pos2::new(time_rect.left() - 8.0, bottom_overlay.top() + 28.0),
                    );
                    if progress_rect.width() >= 20.0 {
                        painter.rect_filled(progress_rect, 4.0, Color32::from_rgb(55, 55, 62));
                        let duration = video_playback.duration.unwrap_or(0.0);
                        let progress = if duration.is_finite() && duration > 0.0 {
                            (video_playback.time_pos.unwrap_or(0.0) / duration).clamp(0.0, 1.0)
                        } else {
                            0.0
                        };
                        let filled = Rect::from_min_size(
                            progress_rect.min,
                            Vec2::new(
                                progress_rect.width() * progress as f32,
                                progress_rect.height(),
                            ),
                        );
                        painter.rect_filled(filled, 4.0, Color32::from_rgb(74, 158, 255));
                        let seek_response = ui
                            .interact(
                                progress_rect.expand2(Vec2::new(0.0, 6.0)),
                                ui.id().with(("video_seek", id.0)),
                                Sense::click_and_drag(),
                            )
                            .on_hover_cursor(CursorIcon::PointingHand)
                            .on_hover_text("Seek");
                        if supports_seek_preview
                            && duration.is_finite()
                            && duration > 0.0
                            && seek_response.hovered()
                        {
                            if let Some(pointer) = input.hover_pos {
                                let fraction = ((pointer.x - progress_rect.left())
                                    / progress_rect.width())
                                .clamp(0.0, 1.0);
                                let hover_time = duration * fraction as f64;
                                self.pending_video_actions
                                    .push((id, VideoAction::RequestSeekPreview(hover_time)));

                                let popup_size = Vec2::new(
                                    168.0_f32.min((screen_rect.width() - 8.0).max(80.0)),
                                    112.0_f32.min((screen_rect.height() - 52.0).max(64.0)),
                                );
                                let popup_x = (pointer.x - popup_size.x * 0.5).clamp(
                                    screen_rect.left() + 4.0,
                                    (screen_rect.right() - popup_size.x - 4.0)
                                        .max(screen_rect.left() + 4.0),
                                );
                                let popup_y = (progress_rect.top() - popup_size.y - 8.0)
                                    .max(screen_rect.top() + 44.0);
                                let popup_rect =
                                    Rect::from_min_size(Pos2::new(popup_x, popup_y), popup_size);
                                painter.rect_filled(popup_rect, 6.0, Color32::from_rgb(20, 20, 24));
                                painter.rect_stroke(
                                    popup_rect,
                                    6.0,
                                    Stroke::new(1.0, Color32::from_rgb(90, 90, 98)),
                                );

                                let image_rect = Rect::from_min_max(
                                    popup_rect.min + Vec2::splat(4.0),
                                    Pos2::new(popup_rect.right() - 4.0, popup_rect.bottom() - 22.0),
                                );
                                let (texture_id, matches_time) = preview_manager
                                    .get_mut(id)
                                    .map_or((None, false), |preview| {
                                        let matches_time = preview
                                            .seek_preview_time()
                                            .is_some_and(|time| (time - hover_time).abs() <= 0.5);
                                        (
                                            preview
                                                .get_seek_preview_texture(ctx)
                                                .map(|texture| texture.id()),
                                            matches_time,
                                        )
                                    });
                                if let Some(texture_id) = texture_id {
                                    painter.image(
                                        texture_id,
                                        image_rect,
                                        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                                        Color32::WHITE,
                                    );
                                    if !matches_time {
                                        painter.rect_filled(
                                            image_rect,
                                            0.0,
                                            Color32::from_black_alpha(90),
                                        );
                                        painter.text(
                                            image_rect.center(),
                                            egui::Align2::CENTER_CENTER,
                                            "Loading preview…",
                                            egui::FontId::proportional(11.0),
                                            Color32::from_rgb(210, 210, 216),
                                        );
                                    }
                                } else {
                                    painter.text(
                                        image_rect.center(),
                                        egui::Align2::CENTER_CENTER,
                                        "Loading preview…",
                                        egui::FontId::proportional(11.0),
                                        Color32::from_rgb(150, 150, 158),
                                    );
                                }
                                painter.text(
                                    Pos2::new(popup_rect.center().x, popup_rect.bottom() - 11.0),
                                    egui::Align2::CENTER_CENTER,
                                    format_time(Some(hover_time)),
                                    egui::FontId::monospace(10.0),
                                    Color32::WHITE,
                                );
                            }
                        }
                        if controls_enabled
                            && duration.is_finite()
                            && duration > 0.0
                            && (seek_response.clicked() || seek_response.dragged())
                        {
                            if let Some(pointer) = seek_response.interact_pointer_pos() {
                                let fraction = ((pointer.x - progress_rect.left())
                                    / progress_rect.width())
                                .clamp(0.0, 1.0);
                                self.pending_video_actions.push((
                                    id,
                                    VideoAction::SeekAbsolute(duration * fraction as f64),
                                ));
                            }
                        }
                    }
                }
            }

            // Muted badge stays visible even without hover so silent tiles
            // are recognizable at a glance.
            if show_overlays && ((is_browser && muted && !pointer_over_tile) || (is_video && muted))
            {
                let badge_rect = Rect::from_min_size(
                    screen_rect.right_top()
                        + Vec2::new(
                            if is_video && pointer_over_tile {
                                -58.0
                            } else {
                                -30.0
                            },
                            8.0,
                        ),
                    Vec2::splat(22.0),
                );
                painter.rect_filled(
                    badge_rect,
                    6.0,
                    Color32::from_rgba_unmultiplied(0, 0, 0, 160),
                );
                painter.text(
                    badge_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    egui_phosphor::regular::SPEAKER_SLASH,
                    egui::FontId::proportional(12.0),
                    Color32::from_rgb(255, 150, 100),
                );
            }

            if show_overlays && manually_frozen {
                let badge_rect = Rect::from_min_size(
                    screen_rect.left_top() + Vec2::new(8.0, 8.0),
                    Vec2::new(72.0, 24.0),
                );
                painter.rect_filled(
                    badge_rect,
                    7.0,
                    Color32::from_rgba_unmultiplied(20, 25, 32, 220),
                );
                painter.rect_stroke(
                    badge_rect,
                    7.0,
                    Stroke::new(1.0, Color32::from_rgb(104, 174, 255)),
                );
                painter.text(
                    badge_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    format!("{}  Frozen", egui_phosphor::regular::SNOWFLAKE),
                    egui::FontId::proportional(10.0),
                    Color32::from_rgb(210, 230, 255),
                );
            }

            // Minimal Void: Only show border when selected (thin blue accent);
            // green accent marks the browser tile currently in interaction mode.
            // Tile focus already fills the canvas, so the chrome is noise.
            if show_overlays && !self.is_focusing_tile() {
                if self.interactive_browser == Some(id) {
                    painter.rect_stroke(
                        screen_rect,
                        8.0,
                        Stroke::new(2.0, Color32::from_rgb(207, 161, 57)),
                    );
                } else if self.selection.contains(&id) {
                    painter.rect_stroke(
                        screen_rect,
                        8.0,
                        Stroke::new(2.0, Color32::from_rgb(74, 158, 255)),
                    );
                }
            }

            let secondary_clicked = context_response.secondary_clicked();

            // Handle click to select
            if preview_response.clicked() {
                if input.ctrl {
                    if self.selection.contains(&id) {
                        self.selection.retain(|&x| x != id);
                    } else {
                        self.selection.push(id);
                    }
                } else {
                    self.selection = vec![id];
                }
            }

            // A context click on an unselected tile makes it the target;
            // clicking any selected tile keeps the whole group selected.
            if secondary_clicked && !self.selection.contains(&id) {
                self.selection = vec![id];
            }

            // Handle double-click: browsers enter interaction mode (the app
            // consumes last_double_clicked); other previews focus their
            // source window.
            if preview_response.double_clicked() && !manually_frozen {
                if is_video {
                    self.pending_video_actions
                        .push((id, VideoAction::SetPaused(!video_playback.paused)));
                } else if !is_playlist {
                    self.last_double_clicked = Some(id);
                }
                if !is_browser && !is_video && !is_playlist {
                    if let Some(preview) = preview_manager.get(id) {
                        if let Some(ref handle) = preview.window_handle {
                            #[cfg(windows)]
                            unsafe {
                                let hwnd = HWND(handle.hwnd as *mut _);
                                let _ = ShowWindow(hwnd, SW_RESTORE);
                                let _ = SetForegroundWindow(hwnd);
                            }
                        }
                    }
                }
            }

            // Handle drag start and remember screen-space origins for pinned tiles.
            if preview_response.drag_started()
                && !input.alt
                && !input.middle_down
                && !pinned_pointer_active
            {
                self.preview_dragging = true;
                self.pinned_drag_origins.clear();

                let dragged_ids: Vec<PreviewId> = if self.selection.contains(&id) {
                    self.selection.clone()
                } else {
                    vec![id]
                };

                for sel_id in dragged_ids {
                    if let Some(preview) = preview_manager.get(sel_id) {
                        if let Some(pin) = preview.viewport_pin {
                            self.pinned_drag_origins.push((sel_id, pin));
                        }
                    }
                }
            }

            // Handle drag to move (only when not panning with Alt or middle mouse)
            // Resize is handled separately in draw_and_interact_selection()
            if preview_response.dragged()
                && !input.alt
                && !input.middle_down
                && !pinned_pointer_active
            {
                // Only move if we're not in a resize operation
                if self.drag_state.is_none() {
                    let screen_delta = preview_response.drag_delta();
                    let canvas_delta = screen_delta / self.zoom;

                    let dragged_ids = if self.selection.contains(&id) {
                        self.selection.clone()
                    } else {
                        vec![id]
                    };
                    // Pinned tiles derive every frame from their original pin
                    // because egui reports a cumulative drag delta. Ordinary
                    // canvas tiles retain their existing movement behavior.
                    for sel_id in dragged_ids {
                        let pinned_origin = self
                            .pinned_drag_origins
                            .iter()
                            .find(|(drag_id, _)| *drag_id == sel_id)
                            .map(|(_, pin)| *pin);
                        if let Some(pin) = pinned_origin {
                            if let Some(preview) = preview_manager.get_mut(sel_id) {
                                preview.viewport_pin = Some(ViewportPin::from_rect(
                                    pin.rect(canvas_rect).translate(screen_delta),
                                    canvas_rect,
                                ));
                            }
                        } else {
                            if let Some(preview) = preview_manager.get_mut(sel_id) {
                                preview.translate(canvas_delta);
                            }
                        }
                    }
                }
            }

            // Tiles stop exactly where the pointer is released.
            if preview_response.drag_stopped() && self.preview_dragging {
                self.preview_dragging = false;
                self.pinned_drag_origins.clear();
            }

            // Context menu for preview
            let context_menu_interaction_active =
                context_response.context_menu_opened() || secondary_target;
            show_context_menu_if(&context_response, context_menu_interaction_active, |ui| {
                ui.set_max_width(280.0);
                ui.label(egui::RichText::new(compact_title(title, 42)).strong());
                ui.separator();

                let selected_ids = if self.selection.contains(&id) {
                    self.selection.clone()
                } else {
                    vec![id]
                };
                let any_live = selected_ids.iter().any(|selected_id| {
                    preview_manager
                        .get(*selected_id)
                        .is_some_and(|preview| !preview.manually_frozen)
                });
                let any_frozen = selected_ids.iter().any(|selected_id| {
                    preview_manager
                        .get(*selected_id)
                        .is_some_and(|preview| preview.manually_frozen)
                });
                let group = selected_ids.len() > 1;
                if any_live
                    && ui
                        .button(if group {
                            "Freeze Selected"
                        } else {
                            "Freeze Tile"
                        })
                        .clicked()
                {
                    self.queue_activity_for_selection(&selected_ids, TileActivityAction::Freeze);
                    ui.close_menu();
                }
                if any_frozen
                    && ui
                        .button(if group {
                            "Resume Selected"
                        } else {
                            "Resume Tile"
                        })
                        .clicked()
                {
                    self.queue_activity_for_selection(&selected_ids, TileActivityAction::Resume);
                    ui.close_menu();
                }
                ui.separator();

                if !is_media && !is_playlist {
                    ui.label("Frame Rate:");
                    for preset in [FpsPreset::Low, FpsPreset::Medium, FpsPreset::High] {
                        let is_current = current_preset == preset;
                        let label = if is_current {
                            format!("  {} ✓", preset.label())
                        } else {
                            format!("  {}", preset.label())
                        };

                        if ui.selectable_label(is_current, label).clicked() {
                            self.pending_fps_changes.push(PendingFpsChange {
                                preview_id: id,
                                new_fps: preset,
                            });
                            ui.close_menu();
                        }
                    }
                    ui.separator();
                }

                if is_window_capture {
                    let native_size = ui
                        .add_enabled(
                            source_frame_size.is_some(),
                            egui::Button::new("Native Size (1:1)"),
                        )
                        .on_hover_text(
                            "Resize this tile so every captured source pixel maps to one screen pixel",
                        );
                    if native_size.clicked() {
                        if let Some(source_size) = source_frame_size {
                            self.set_native_capture_size(
                                id,
                                source_size,
                                canvas_rect,
                                ctx.pixels_per_point(),
                                preview_manager,
                            );
                        }
                        ui.close_menu();
                    }
                    ui.separator();
                }

                if is_media {
                    if let Some(error) = preview_manager
                        .get(id)
                        .and_then(|preview| preview.capture_error.as_deref())
                    {
                        ui.label(egui::RichText::new(error).small().weak());
                    }
                    if ui.button("Retry Image").clicked() {
                        self.pending_media_actions.push((id, MediaAction::Retry));
                        ui.close_menu();
                    }
                    if ui.button("Locate Image…").clicked() {
                        self.pending_media_actions.push((id, MediaAction::Locate));
                        ui.close_menu();
                    }
                    ui.separator();
                }

                if ui
                    .selectable_label(viewport_pin.is_some(), "Pin to Viewport")
                    .on_hover_text("Keep this tile fixed above the canvas while panning or zooming")
                    .clicked()
                {
                    self.toggle_viewport_pin(id, canvas_rect, preview_manager);
                    ui.close_menu();
                }

                if is_browser
                    && ui
                        .selectable_label(lock_aspect_ratio, "Lock Aspect Ratio")
                        .on_hover_text(
                            "Turn this off to resize the browser into portrait or any custom shape",
                        )
                        .clicked()
                {
                    self.toggle_aspect_ratio_lock(id, preview_manager);
                    ui.close_menu();
                }

                if ui
                    .selectable_label(left_click_disabled, "Disable Left Click")
                    .on_hover_text("Let primary clicks and drags pass through to tiles underneath")
                    .clicked()
                {
                    self.toggle_left_click_disabled(id, preview_manager);
                    ui.close_menu();
                }
                ui.separator();

                if is_playlist {
                    ui.label(egui::RichText::new("Folder playlist tile").weak());
                    if ui.button("Previous video").clicked() {
                        self.pending_playlist_actions
                            .push((id, PlaylistAction::Previous));
                        ui.close_menu();
                    }
                    if ui.button("Next video").clicked() {
                        self.pending_playlist_actions
                            .push((id, PlaylistAction::Next));
                        ui.close_menu();
                    }
                    if ui.button("Rescan folder").clicked() {
                        self.pending_playlist_actions
                            .push((id, PlaylistAction::Rescan));
                        ui.close_menu();
                    }
                    let (autoplay, shuffle, repeat) = preview_manager
                        .get(id)
                        .and_then(|preview| preview.folder_playlist.as_ref())
                        .map(|playlist| (playlist.autoplay, playlist.shuffle, playlist.repeat))
                        .unwrap_or((true, false, false));
                    if ui.selectable_label(autoplay, "Autoplay next").clicked() {
                        self.pending_playlist_actions
                            .push((id, PlaylistAction::ToggleAutoplay));
                        ui.close_menu();
                    }
                    if ui.selectable_label(shuffle, "Shuffle").clicked() {
                        self.pending_playlist_actions
                            .push((id, PlaylistAction::ToggleShuffle));
                        ui.close_menu();
                    }
                    if ui.selectable_label(repeat, "Repeat playlist").clicked() {
                        self.pending_playlist_actions
                            .push((id, PlaylistAction::ToggleRepeat));
                        ui.close_menu();
                    }
                } else if is_media {
                    ui.label(egui::RichText::new("Image / animated GIF tile").weak());
                    ui.separator();
                    ui.menu_button("Crop", |ui| {
                        if has_crop && ui.button("Clear Crop").clicked() {
                            self.set_preview_crop(id, None, canvas_rect, preview_manager);
                            ui.close_menu();
                        }
                        ui.separator();
                        ui.label(
                            egui::RichText::new("Tip: Alt+drag edges or corners")
                                .weak()
                                .small(),
                        );
                    });
                } else if is_browser {
                    let browser_ready =
                        *browser_status == BrowserTileStatus::Ready && !manually_frozen;
                    if !browser_ready {
                        ui.label(egui::RichText::new("Browser preview is starting…").weak());
                    }
                    if ui
                        .add_enabled(
                            browser_ready && !left_click_disabled,
                            egui::Button::new("Interact"),
                        )
                        .clicked()
                    {
                        self.last_double_clicked = Some(id);
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(
                            browser_ready,
                            egui::Button::new(if muted { "Unmute" } else { "Mute" }),
                        )
                        .clicked()
                    {
                        self.pending_browser_actions
                            .push((id, BrowserAction::ToggleMute));
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(browser_ready, egui::Button::new("Reload"))
                        .clicked()
                    {
                        self.pending_browser_actions
                            .push((id, BrowserAction::Reload));
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(browser_ready, egui::Button::new("Change URL..."))
                        .clicked()
                    {
                        self.pending_browser_actions
                            .push((id, BrowserAction::EditUrl));
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(browser_ready, egui::Button::new("Copy URL"))
                        .clicked()
                    {
                        self.pending_browser_actions
                            .push((id, BrowserAction::CopyUrl));
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(browser_ready, egui::Button::new("Open in Default Browser"))
                        .clicked()
                    {
                        self.pending_browser_actions
                            .push((id, BrowserAction::OpenExternal));
                        ui.close_menu();
                    }
                    ui.separator();
                    ui.menu_button("Crop", |ui| {
                        if has_crop && ui.button("Clear Crop").clicked() {
                            self.set_preview_crop(id, None, canvas_rect, preview_manager);
                            ui.close_menu();
                        }
                        ui.separator();
                        ui.label(
                            egui::RichText::new("Tip: Alt+drag edges or corners")
                                .weak()
                                .small(),
                        );
                    });
                } else if is_video {
                    let controls_enabled = !manually_frozen
                        && video_playback.connected
                        && !matches!(video_status, VideoTileStatus::Failed(_));
                    if !controls_enabled {
                        ui.label(egui::RichText::new("Video controls are unavailable").weak());
                    }
                    if ui
                        .add_enabled(!manually_frozen, egui::Button::new("Reload"))
                        .clicked()
                    {
                        self.pending_video_actions.push((id, VideoAction::Reload));
                        ui.close_menu();
                    }
                    ui.add_enabled_ui(controls_enabled, |ui| {
                        if ui
                            .button(if video_playback.paused {
                                "Play"
                            } else {
                                "Pause"
                            })
                            .clicked()
                        {
                            self.pending_video_actions
                                .push((id, VideoAction::SetPaused(!video_playback.paused)));
                            ui.close_menu();
                        }
                        if ui.button(if muted { "Unmute" } else { "Mute" }).clicked() {
                            self.pending_video_actions
                                .push((id, VideoAction::ToggleMute));
                            ui.close_menu();
                        }

                        let mut volume = video_playback.volume;
                        if ui
                            .add(egui::Slider::new(&mut volume, 0.0..=100.0).text("Volume"))
                            .changed()
                        {
                            self.pending_video_actions
                                .push((id, VideoAction::SetVolume(volume)));
                        }

                        if ui
                            .selectable_label(video_playback.looping, "Loop")
                            .clicked()
                        {
                            self.pending_video_actions
                                .push((id, VideoAction::ToggleLoop));
                            ui.close_menu();
                        }

                        ui.menu_button(format!("Speed: {}×", video_playback.speed), |ui| {
                            for speed in [0.5, 0.75, 1.0, 1.25, 1.5, 2.0] {
                                let selected = (video_playback.speed - speed).abs() < 0.001;
                                if ui.selectable_label(selected, format!("{speed}×")).clicked() {
                                    self.pending_video_actions
                                        .push((id, VideoAction::SetSpeed(speed)));
                                    ui.close_menu();
                                }
                            }
                        });

                        ui.menu_button("Audio Track", |ui| {
                            let audio_tracks: Vec<_> = video_playback
                                .tracks
                                .iter()
                                .filter(|track| track.kind == "audio")
                                .collect();
                            if audio_tracks.is_empty() {
                                ui.label(egui::RichText::new("No audio tracks").weak());
                            }
                            for track in audio_tracks {
                                let label = track
                                    .title
                                    .clone()
                                    .or_else(|| track.language.clone())
                                    .unwrap_or_else(|| format!("Track {}", track.id));
                                if ui
                                    .selectable_label(
                                        video_playback.audio_track == Some(track.id),
                                        label,
                                    )
                                    .clicked()
                                {
                                    self.pending_video_actions
                                        .push((id, VideoAction::SelectAudioTrack(track.id)));
                                    ui.close_menu();
                                }
                            }
                        });

                        ui.menu_button("Subtitles", |ui| {
                            if ui
                                .selectable_label(video_playback.subtitle_track.is_none(), "Off")
                                .clicked()
                            {
                                self.pending_video_actions
                                    .push((id, VideoAction::SelectSubtitleTrack(None)));
                                ui.close_menu();
                            }
                            for track in video_playback
                                .tracks
                                .iter()
                                .filter(|track| track.kind == "sub")
                            {
                                let label = track
                                    .title
                                    .clone()
                                    .or_else(|| track.language.clone())
                                    .unwrap_or_else(|| format!("Track {}", track.id));
                                if ui
                                    .selectable_label(
                                        video_playback.subtitle_track == Some(track.id),
                                        label,
                                    )
                                    .clicked()
                                {
                                    self.pending_video_actions.push((
                                        id,
                                        VideoAction::SelectSubtitleTrack(Some(track.id)),
                                    ));
                                    ui.close_menu();
                                }
                            }
                        });
                    });

                    if video_status_needs_settings(video_status)
                        && ui.button("Open Settings").clicked()
                    {
                        self.pending_video_actions
                            .push((id, VideoAction::OpenSettings));
                        ui.close_menu();
                    }

                    ui.separator();
                    ui.menu_button("Crop", |ui| {
                        if !is_spout_capture && ui.button("Select Region...").clicked() {
                            self.pending_region_select = Some(id);
                            ui.close_menu();
                        }
                        if has_crop && ui.button("Clear Crop").clicked() {
                            self.set_preview_crop(id, None, canvas_rect, preview_manager);
                            ui.close_menu();
                        }
                    });
                } else {
                    // Crop section
                    ui.menu_button("Crop", |ui| {
                        if !is_spout_capture && ui.button("Select Region...").clicked() {
                            self.pending_region_select = Some(id);
                            ui.close_menu();
                        }

                        if has_crop && ui.button("Clear Crop").clicked() {
                            self.set_preview_crop(id, None, canvas_rect, preview_manager);
                            ui.close_menu();
                        }

                        ui.separator();
                        ui.label(
                            egui::RichText::new("Tip: Alt+drag corners to fine-tune")
                                .weak()
                                .small(),
                        );
                    });
                }

                ui.separator();

                if ui
                    .add_enabled(
                        viewport_pin.is_none(),
                        egui::Button::new("Focus on This Tile"),
                    )
                    .clicked()
                {
                    self.focus_on_tile(id, rect, canvas_rect);
                    ui.close_menu();
                }

                if ui.button("Bring to Front").clicked() {
                    preview_manager.bring_to_front(id);
                    ui.close_menu();
                }

                if ui.button("Send to Back").clicked() {
                    preview_manager.send_to_back(id);
                    ui.close_menu();
                }

                ui.separator();

                if ui.button("Remove").clicked() {
                    capture_coordinator.stop_capture(id);
                    preview_manager.start_removal(id);
                    self.selection.retain(|&x| x != id);
                    ui.close_menu();
                }
            });
        }
        self.tile_scratch = preview_info;

        // Keep repainting while any preview is spawning in, fading out, or
        // still waiting on its first frame so the animations stay smooth.
        if any_spawn_or_remove_animating {
            ctx.request_repaint();
        }
        if window_capture_connecting {
            ctx.request_repaint_after(WINDOW_CONNECTING_REPAINT_INTERVAL);
        }
        if transparent_browser_waiting {
            ctx.request_repaint_after(TRANSPARENT_BROWSER_REPAINT_INTERVAL);
        }
    }

    fn paint_folder_playlist(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        rect: Rect,
        preview_manager: &mut PreviewManager,
        id: PreviewId,
        alpha: u8,
    ) {
        let z = playlist_zoom(self.zoom);
        let s = |value: f32| playlist_px(value, z);
        let corner = s(PLAYLIST_CORNER);
        let clipped = painter.with_clip_rect(rect);
        clipped.rect_filled(
            rect,
            corner,
            Color32::from_rgba_unmultiplied(16, 17, 21, alpha),
        );

        let header_height = s(PLAYLIST_HEADER_HEIGHT).min(rect.height());
        let header = Rect::from_min_size(rect.min, Vec2::new(rect.width(), header_height));
        clipped.rect_filled(
            header,
            egui::Rounding {
                nw: corner,
                ne: corner,
                sw: 0.0,
                se: 0.0,
            },
            Color32::from_rgba_unmultiplied(22, 24, 29, alpha),
        );

        let Some(preview) = preview_manager.get_mut(id) else {
            return;
        };
        let title = preview.title.clone();
        let Some(playlist) = preview.folder_playlist.as_mut() else {
            return;
        };

        let count = playlist.entries.len();
        let count_text = if count == 1 {
            "1 video".to_owned()
        } else {
            format!("{count} videos")
        };

        let icon_size = s(28.0);
        let icon_rect = Rect::from_center_size(
            Pos2::new(header.left() + s(22.0), header.center().y),
            Vec2::splat(icon_size),
        );
        clipped.rect_filled(
            icon_rect,
            s(8.0),
            Color32::from_rgba_unmultiplied(207, 161, 57, 38),
        );
        clipped.text(
            icon_rect.center(),
            egui::Align2::CENTER_CENTER,
            egui_phosphor::regular::FOLDER_OPEN,
            playlist_font(15.0, z),
            Color32::from_rgb(138, 196, 108),
        );

        let text_left = icon_rect.right() + s(10.0);
        let text_right = header.right() - s(40.0);
        paint_truncated_text(
            &clipped,
            Pos2::new(text_left, header.center().y - s(8.0)),
            egui::Align2::LEFT_CENTER,
            &title,
            playlist_font(13.5, z),
            Color32::from_rgb(236, 236, 240),
            (text_right - text_left).max(8.0),
        );
        paint_truncated_text(
            &clipped,
            Pos2::new(text_left, header.center().y + s(10.0)),
            egui::Align2::LEFT_CENTER,
            &count_text,
            playlist_font(10.5, z),
            Color32::from_rgb(132, 136, 148),
            (text_right - text_left).max(8.0),
        );

        let toolbar_top = header.bottom();
        let toolbar_height = s(PLAYLIST_TOOLBAR_HEIGHT);
        let toolbar = Rect::from_min_size(
            Pos2::new(rect.left(), toolbar_top),
            Vec2::new(
                rect.width(),
                toolbar_height.min((rect.bottom() - toolbar_top).max(0.0)),
            ),
        );
        if toolbar.height() > 1.0 {
            clipped.rect_filled(
                toolbar,
                0.0,
                Color32::from_rgba_unmultiplied(18, 19, 24, alpha),
            );
            clipped.line_segment(
                [toolbar.left_top(), toolbar.right_top()],
                Stroke::new(
                    s(1.0).max(1.0),
                    Color32::from_rgba_unmultiplied(48, 50, 58, alpha),
                ),
            );
        }

        let controls = [
            (
                egui_phosphor::regular::SKIP_BACK,
                PlaylistAction::Previous,
                false,
                "Previous",
            ),
            (
                egui_phosphor::regular::SKIP_FORWARD,
                PlaylistAction::Next,
                false,
                "Next",
            ),
            (
                egui_phosphor::regular::SHUFFLE,
                PlaylistAction::ToggleShuffle,
                playlist.shuffle,
                "Shuffle",
            ),
            (
                egui_phosphor::regular::REPEAT,
                PlaylistAction::ToggleRepeat,
                playlist.repeat,
                "Repeat playlist",
            ),
            (
                egui_phosphor::regular::PLAYLIST,
                PlaylistAction::ToggleAutoplay,
                playlist.autoplay,
                "Autoplay next",
            ),
        ];
        let button_size = s(26.0);
        let button_gap = s(6.0);
        let controls_width = controls.len() as f32 * button_size
            + (controls.len().saturating_sub(1) as f32) * button_gap;
        let controls_left = toolbar.center().x - controls_width * 0.5;
        for (index, (icon, action, active, tip)) in controls.into_iter().enumerate() {
            let button = Rect::from_center_size(
                Pos2::new(
                    controls_left + button_size * 0.5 + index as f32 * (button_size + button_gap),
                    toolbar.center().y,
                ),
                Vec2::splat(button_size),
            );
            let response = ui
                .interact(
                    button,
                    ui.id().with(("playlist_control", id.0, index)),
                    Sense::click(),
                )
                .on_hover_text(tip);
            if response.hovered() {
                ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
            }
            if response.hovered() || active {
                clipped.rect_filled(
                    button,
                    s(7.0),
                    if active {
                        Color32::from_rgba_unmultiplied(74, 158, 255, 64)
                    } else {
                        Color32::from_rgba_unmultiplied(255, 255, 255, 22)
                    },
                );
            }
            clipped.text(
                button.center(),
                egui::Align2::CENTER_CENTER,
                icon,
                playlist_font(13.0, z),
                if active {
                    Color32::from_rgb(158, 206, 255)
                } else {
                    Color32::from_rgb(188, 190, 198)
                },
            );
            if response.clicked() {
                self.pending_playlist_actions.push((id, action));
            }
        }

        let row_height = s(PLAYLIST_ROW_HEIGHT);
        let list_rect = Rect::from_min_max(
            Pos2::new(rect.left(), header.bottom() + toolbar.height()),
            rect.max,
        );
        let list = clipped.with_clip_rect(list_rect);
        let list_height_canvas = list_rect.height() / z;
        let max_scroll =
            (playlist.entries.len() as f32 * PLAYLIST_ROW_HEIGHT - list_height_canvas).max(0.0);
        playlist.scroll_offset = playlist.scroll_offset.clamp(0.0, max_scroll);
        let first = (playlist.scroll_offset / PLAYLIST_ROW_HEIGHT).floor() as usize;
        let y_remainder = (playlist.scroll_offset % PLAYLIST_ROW_HEIGHT) * z;
        let selected = playlist.selected.clone();

        if let Some(error) = playlist.error.as_ref() {
            paint_truncated_text(
                &list,
                list_rect.center(),
                egui::Align2::CENTER_CENTER,
                error,
                playlist_font(12.0, z),
                Color32::from_rgb(220, 148, 132),
                list_rect.width() - s(24.0),
            );
        } else if playlist.entries.is_empty() {
            list.text(
                list_rect.center() - Vec2::new(0.0, s(10.0)),
                egui::Align2::CENTER_CENTER,
                egui_phosphor::regular::VIDEO,
                playlist_font(22.0, z),
                Color32::from_rgb(78, 82, 92),
            );
            paint_truncated_text(
                &list,
                list_rect.center() + Vec2::new(0.0, s(16.0)),
                egui::Align2::CENTER_CENTER,
                "No videos in this folder",
                playlist_font(12.0, z),
                Color32::from_rgb(132, 136, 148),
                list_rect.width() - s(24.0),
            );
        }

        for (index, entry) in playlist.entries.iter_mut().enumerate().skip(first) {
            let top = list_rect.top() + (index - first) as f32 * row_height - y_remainder;
            let row = Rect::from_min_size(
                Pos2::new(list_rect.left(), top),
                Vec2::new(list_rect.width(), row_height),
            );
            if row.top() >= list_rect.bottom() {
                break;
            }
            if row.bottom() <= list_rect.top() {
                continue;
            }

            let active = selected.as_ref().is_some_and(|path| path == &entry.path);
            let hit = row.intersect(list_rect);
            let response = ui
                .interact(
                    hit,
                    ui.id().with(("playlist_row", id.0, index)),
                    Sense::click(),
                )
                .on_hover_text(&entry.name);
            if response.hovered() {
                ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
            }

            let pad = s(8.0);
            let card = Rect::from_min_max(
                Pos2::new(row.left() + pad, row.top() + s(4.0)),
                Pos2::new(row.right() - pad, row.bottom() - s(4.0)),
            );
            if active || response.hovered() {
                list.rect_filled(
                    card,
                    s(8.0),
                    if active {
                        Color32::from_rgba_unmultiplied(74, 158, 255, 36)
                    } else {
                        Color32::from_rgba_unmultiplied(255, 255, 255, 12)
                    },
                );
            }
            if active {
                list.rect_filled(
                    Rect::from_min_size(
                        Pos2::new(card.left(), card.top() + s(8.0)),
                        Vec2::new(s(3.0), card.height() - s(16.0)),
                    ),
                    s(1.5),
                    Color32::from_rgb(207, 161, 57),
                );
            }

            let thumb = Rect::from_min_size(
                Pos2::new(card.left() + s(10.0), card.center().y - s(20.0)),
                Vec2::new(s(72.0), s(40.0)),
            );
            list.rect_filled(thumb, s(6.0), Color32::from_rgb(28, 30, 36));
            if let Some(texture) = &entry.thumbnail {
                let thumb_clip = list.with_clip_rect(thumb);
                thumb_clip.image(
                    texture.id(),
                    thumb,
                    cover_uv(texture.size_vec2(), thumb.size()),
                    Color32::from_white_alpha(alpha),
                );
            } else {
                list.text(
                    thumb.center(),
                    egui::Align2::CENTER_CENTER,
                    egui_phosphor::regular::VIDEO,
                    playlist_font(16.0, z),
                    Color32::from_rgb(88, 92, 102),
                );
                if entry.thumbnail_state == crate::playlist::ThumbnailState::Idle {
                    entry.thumbnail_state = crate::playlist::ThumbnailState::Loading;
                    self.pending_playlist_actions
                        .push((id, PlaylistAction::RequestThumbnail(entry.path.clone())));
                }
            }

            let (stem, extension) = playlist_entry_labels(&entry.name);
            let text_x = thumb.right() + s(10.0);
            let text_max = card.right() - s(12.0);
            paint_truncated_text(
                &list,
                Pos2::new(text_x, card.center().y - s(8.0)),
                egui::Align2::LEFT_CENTER,
                stem,
                playlist_font(12.5, z),
                if active {
                    Color32::WHITE
                } else {
                    Color32::from_rgb(214, 216, 222)
                },
                (text_max - text_x).max(8.0),
            );
            let meta = if active {
                "Now playing".to_owned()
            } else if let Some(extension) = extension {
                format!("{:02}  ·  {}", index + 1, extension.to_ascii_uppercase())
            } else {
                format!("{:02}", index + 1)
            };
            paint_truncated_text(
                &list,
                Pos2::new(text_x, card.center().y + s(10.0)),
                egui::Align2::LEFT_CENTER,
                &meta,
                playlist_font(10.0, z),
                if active {
                    Color32::from_rgb(138, 196, 108)
                } else {
                    Color32::from_rgb(118, 122, 132)
                },
                (text_max - text_x).max(8.0),
            );
            if response.clicked() {
                self.pending_playlist_actions
                    .push((id, PlaylistAction::Select(entry.path.clone())));
            }
        }

        if max_scroll > 0.0 {
            let fraction = (list_height_canvas
                / (playlist.entries.len() as f32 * PLAYLIST_ROW_HEIGHT))
                .clamp(0.08, 1.0);
            let track = Rect::from_min_max(
                Pos2::new(rect.right() - s(7.0), list_rect.top() + s(8.0)),
                Pos2::new(rect.right() - s(3.0), list_rect.bottom() - s(8.0)),
            );
            list.rect_filled(
                track,
                s(2.0),
                Color32::from_rgba_unmultiplied(255, 255, 255, 16),
            );
            let thumb_height = track.height() * fraction;
            let thumb_top = track.top()
                + (track.height() - thumb_height) * (playlist.scroll_offset / max_scroll);
            list.rect_filled(
                Rect::from_min_size(
                    Pos2::new(track.left(), thumb_top),
                    Vec2::new(track.width(), thumb_height),
                ),
                s(2.0),
                Color32::from_rgb(92, 96, 108),
            );
        }
    }

    /// Draw the background grid - Minimal Void: very subtle
    fn draw_grid(&self, painter: &egui::Painter, canvas_rect: Rect) {
        let viewport = self.get_viewport(canvas_rect);
        // Minimal Void: very subtle grid (opacity 5 instead of 15)
        let grid_color = Color32::from_rgba_unmultiplied(255, 255, 255, 5);

        let screen_grid_size = self.grid_size * self.zoom;

        if screen_grid_size < 10.0 {
            return;
        }

        let start_x = (viewport.min.x / self.grid_size).floor() * self.grid_size;
        let start_y = (viewport.min.y / self.grid_size).floor() * self.grid_size;

        let mut x = start_x;
        while x < viewport.max.x {
            let screen_x = self.canvas_to_screen(Pos2::new(x, 0.0), canvas_rect).x;
            if screen_x >= canvas_rect.min.x && screen_x <= canvas_rect.max.x {
                painter.line_segment(
                    [
                        Pos2::new(screen_x, canvas_rect.min.y),
                        Pos2::new(screen_x, canvas_rect.max.y),
                    ],
                    Stroke::new(1.0, grid_color),
                );
            }
            x += self.grid_size;
        }

        let mut y = start_y;
        while y < viewport.max.y {
            let screen_y = self.canvas_to_screen(Pos2::new(0.0, y), canvas_rect).y;
            if screen_y >= canvas_rect.min.y && screen_y <= canvas_rect.max.y {
                painter.line_segment(
                    [
                        Pos2::new(canvas_rect.min.x, screen_y),
                        Pos2::new(canvas_rect.max.x, screen_y),
                    ],
                    Stroke::new(1.0, grid_color),
                );
            }
            y += self.grid_size;
        }

        // Origin crosshair - Minimal Void: very subtle white instead of red
        let origin_screen = self.canvas_to_screen(Pos2::ZERO, canvas_rect);
        if canvas_rect.contains(origin_screen) {
            let origin_color = Color32::from_rgba_unmultiplied(255, 255, 255, 12);
            painter.line_segment(
                [
                    Pos2::new(origin_screen.x, canvas_rect.min.y),
                    Pos2::new(origin_screen.x, canvas_rect.max.y),
                ],
                Stroke::new(1.0, origin_color),
            );
            painter.line_segment(
                [
                    Pos2::new(canvas_rect.min.x, origin_screen.y),
                    Pos2::new(canvas_rect.max.x, origin_screen.y),
                ],
                Stroke::new(1.0, origin_color),
            );
        }
    }

    /// Minimal Void: Draw floating status indicator in bottom-right corner
    fn draw_floating_status(
        &self,
        painter: &egui::Painter,
        canvas_rect: Rect,
        preview_count: usize,
    ) {
        let status_text = format!("{}%  {}⬚", (self.zoom * 100.0) as i32, preview_count);

        // Position in bottom-right with padding
        let padding = 16.0;
        let badge_width = 80.0;
        let badge_height = 24.0;

        let status_rect = Rect::from_min_size(
            Pos2::new(
                canvas_rect.max.x - badge_width - padding,
                canvas_rect.max.y - badge_height - padding,
            ),
            Vec2::new(badge_width, badge_height),
        );

        // Semi-transparent dark background with rounded corners
        painter.rect_filled(
            status_rect,
            12.0,
            Color32::from_rgba_unmultiplied(0, 0, 0, 150),
        );

        // Status text
        painter.text(
            status_rect.center(),
            egui::Align2::CENTER_CENTER,
            &status_text,
            egui::FontId::proportional(11.0),
            Color32::from_rgb(120, 120, 120),
        );
    }

    /// Empty-canvas hint shown before any preview has been added.
    fn draw_empty_state(&self, painter: &egui::Painter, canvas_rect: Rect) {
        let center = canvas_rect.center();

        painter.text(
            center + Vec2::new(0.0, -18.0),
            egui::Align2::CENTER_CENTER,
            egui_phosphor::regular::APP_WINDOW,
            egui::FontId::proportional(40.0),
            Color32::from_rgb(55, 55, 60),
        );
        painter.text(
            center + Vec2::new(0.0, 22.0),
            egui::Align2::CENTER_CENTER,
            "No windows yet",
            egui::FontId::proportional(15.0),
            Color32::from_rgb(110, 110, 118),
        );
        painter.text(
            center + Vec2::new(0.0, 44.0),
            egui::Align2::CENTER_CENTER,
            "Add a window from the left panel, or right-click for a window, browser, or image tile",
            egui::FontId::proportional(12.0),
            Color32::from_rgb(75, 75, 82),
        );
    }

    pub fn request_removed_undo(&mut self) {
        self.removed_undo_requested = true;
    }

    fn queue_removed_undo(&mut self) {
        if let Some((_, removed)) = self.last_removed.take() {
            self.pending_removed_restore = Some(removed);
        }
    }

    /// Floating "Removed '...' · Undo" toast for the most recently removed preview.
    fn draw_and_interact_undo_toast(&mut self, ui: &mut egui::Ui, canvas_rect: Rect) {
        let Some((removed_at, _)) = self.last_removed.as_ref() else {
            return;
        };

        let age = removed_at.elapsed().as_secs_f32();
        if age >= UNDO_TOAST_SECS {
            return;
        }
        let removed = &self
            .last_removed
            .as_ref()
            .expect("removed preview is present")
            .1;

        // Fade in quickly, fade out over the last half-second.
        let fade_in = (age / 0.15).clamp(0.0, 1.0);
        let fade_out = ((UNDO_TOAST_SECS - age) / 0.5).clamp(0.0, 1.0);
        let fade = fade_in.min(fade_out);
        let bg_alpha = (fade * 220.0) as u8;
        let text_alpha = (fade * 255.0) as u8;

        let info = &removed[0];
        let label = if removed.len() > 1 {
            format!("Removed {} tiles", removed.len())
        } else if info.title.chars().count() > 28 {
            let truncated: String = info.title.chars().take(25).collect();
            format!("Removed \"{}...\"", truncated)
        } else {
            format!("Removed \"{}\"", info.title)
        };

        let padding = 16.0;
        let toast_height = 32.0;
        let toast_width = 230.0;
        let toast_rect = Rect::from_min_size(
            Pos2::new(
                canvas_rect.min.x + padding,
                canvas_rect.max.y - toast_height - padding,
            ),
            Vec2::new(toast_width, toast_height),
        );

        let painter = ui.painter_at(canvas_rect);
        painter.rect_filled(
            toast_rect,
            10.0,
            Color32::from_rgba_unmultiplied(24, 24, 28, bg_alpha),
        );
        painter.text(
            Pos2::new(toast_rect.min.x + 12.0, toast_rect.center().y),
            egui::Align2::LEFT_CENTER,
            &label,
            egui::FontId::proportional(11.5),
            Color32::from_rgba_unmultiplied(210, 210, 215, text_alpha),
        );

        let undo_rect = Rect::from_min_size(
            Pos2::new(toast_rect.max.x - 56.0, toast_rect.min.y + 6.0),
            Vec2::new(48.0, toast_height - 12.0),
        );
        let undo_response = ui.interact(undo_rect, ui.id().with("undo_toast_btn"), Sense::click());
        let undo_color = if undo_response.hovered() {
            Color32::from_rgba_unmultiplied(140, 200, 255, text_alpha)
        } else {
            Color32::from_rgba_unmultiplied(74, 158, 255, text_alpha)
        };
        painter.text(
            undo_rect.center(),
            egui::Align2::CENTER_CENTER,
            "Undo",
            egui::FontId::proportional(12.0),
            undo_color,
        );

        if undo_response.clicked() {
            self.request_removed_undo();
            ui.ctx().request_repaint();
        }

        // Keep repainting while the toast is visible so it can fade out.
        ui.ctx().request_repaint();
    }

    /// Draw selection indicators and interactive resize handles
    fn draw_and_interact_selection(
        &mut self,
        ui: &mut egui::Ui,
        canvas_rect: Rect,
        preview_manager: &mut PreviewManager,
        input: &FrameInput,
    ) {
        let painter = ui.painter_at(canvas_rect);
        let alt_held = input.alt;

        // Collect selection info to avoid borrow issues
        let selection_info: Vec<_> = self
            .selection
            .iter()
            .filter_map(|id| {
                preview_manager.get(*id).and_then(|p| {
                    if p.left_click_disabled {
                        return None;
                    }
                    Some((
                        *id,
                        p.rect(),
                        p.source_aspect_ratio,
                        p.lock_aspect_ratio,
                        p.crop_uv,
                        p.source_frame_size.or(p.frame_size),
                        p.viewport_pin,
                        p.is_window_capture(),
                        p.is_video(),
                        p.is_playlist(),
                    ))
                })
            })
            .collect();

        for (
            id,
            preview_rect,
            aspect_ratio,
            lock_aspect_ratio,
            crop_uv,
            frame_size,
            viewport_pin,
            is_window_capture,
            is_video,
            is_playlist,
        ) in selection_info
        {
            let screen_rect = viewport_pin
                .map(|pin| pin.rect(canvas_rect))
                .unwrap_or_else(|| self.canvas_rect_to_screen(preview_rect, canvas_rect));
            let screen_rect = if is_window_capture {
                pixel_aligned_rect(screen_rect, ui.ctx().pixels_per_point())
            } else {
                screen_rect
            };

            // Minimal Void: Selection border with accent color.
            let border_color = if self.interactive_browser == Some(id) {
                Color32::from_rgb(207, 161, 57) // Gold: live interaction mode
            } else if alt_held && !is_playlist {
                Color32::from_rgb(255, 150, 100) // Orange for crop mode
            } else {
                Color32::from_rgb(74, 158, 255) // #4a9eff blue accent
            };
            painter.rect_stroke(
                screen_rect,
                8.0, // Rounded corners
                Stroke::new(2.0, border_color),
            );

            // Minimal Void: Smaller, more subtle resize handles
            let handle_size = 6.0; // Reduced from 8.0
            let handles = [
                (screen_rect.left_top(), ResizeHandle::TopLeft),
                (screen_rect.center_top(), ResizeHandle::Top),
                (screen_rect.right_top(), ResizeHandle::TopRight),
                (screen_rect.left_center(), ResizeHandle::Left),
                (screen_rect.right_center(), ResizeHandle::Right),
                (screen_rect.left_bottom(), ResizeHandle::BottomLeft),
                (screen_rect.center_bottom(), ResizeHandle::Bottom),
                (screen_rect.right_bottom(), ResizeHandle::BottomRight),
            ];

            for (handle_pos, handle_type) in handles {
                let handle_rect = Rect::from_center_size(handle_pos, Vec2::splat(handle_size));
                let hit_rect =
                    Rect::from_center_size(handle_pos, Vec2::splat(RESIZE_HANDLE_HIT_SIZE));

                // Minimal Void: Clean handles matching selection color
                let handle_fill = if alt_held && !is_playlist {
                    Color32::from_rgb(255, 150, 100) // Orange for crop mode
                } else {
                    Color32::from_rgb(74, 158, 255) // Match accent color
                };
                painter.rect_filled(handle_rect, 3.0, handle_fill);
                // No stroke - cleaner look

                // Create interactive area for this handle
                let handle_response = ui.interact(
                    hit_rect,
                    ui.id().with(("resize_handle", id.0, handle_type as u8)),
                    Sense::drag(),
                );

                // Show cursor on hover
                if handle_response.hovered() {
                    ui.ctx().set_cursor_icon(handle_type.cursor());
                }

                // Handle drag start - check if Alt is held for crop mode.
                if handle_response.drag_started() {
                    if alt_held && (frame_size.is_some() || is_video) && !is_playlist {
                        // Start crop mode
                        let current_crop = crop_uv.unwrap_or((0.0, 0.0, 1.0, 1.0));
                        self.drag_state = Some(DragState::Cropping {
                            id,
                            handle: handle_type,
                            start_mouse: input.interact_pos.unwrap_or(handle_pos),
                            start_rect: if viewport_pin.is_some() {
                                screen_rect
                            } else {
                                preview_rect
                            },
                            screen_space: viewport_pin.is_some(),
                            start_crop_uv: current_crop,
                        });
                    } else {
                        self.drag_state = Some(DragState::Resizing {
                            id,
                            handle: handle_type,
                            start_rect: if viewport_pin.is_some() {
                                screen_rect
                            } else {
                                preview_rect
                            },
                            start_mouse: input.interact_pos.unwrap_or(handle_pos),
                            aspect_ratio: lock_aspect_ratio.then_some(aspect_ratio),
                            screen_space: viewport_pin.is_some(),
                        });
                    }
                }

                // Handle dragging
                if handle_response.dragged() {
                    // Handle resize mode
                    if let Some(DragState::Resizing {
                        id: resize_id,
                        handle,
                        start_rect,
                        start_mouse,
                        aspect_ratio: ar,
                        screen_space,
                    }) = &self.drag_state
                    {
                        if *resize_id == id && *handle == handle_type {
                            if let Some(current_pos) = input.interact_pos {
                                let screen_delta = current_pos - *start_mouse;
                                let delta = if *screen_space {
                                    screen_delta
                                } else {
                                    screen_delta / self.zoom
                                };
                                let new_rect = apply_resize(*handle, *start_rect, delta, *ar);

                                // Apply minimum size
                                let min_size = 100.0;
                                if new_rect.width() >= min_size && new_rect.height() >= min_size {
                                    if let Some(preview) = preview_manager.get_mut(id) {
                                        if *screen_space {
                                            preview.viewport_pin =
                                                Some(ViewportPin::from_rect(new_rect, canvas_rect));
                                        } else {
                                            preview.position = new_rect.min;
                                            preview.size = new_rect.size();
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // Handle crop mode
                    if let Some(DragState::Cropping {
                        id: crop_id,
                        handle,
                        start_mouse,
                        start_rect,
                        screen_space,
                        start_crop_uv,
                    }) = &self.drag_state
                    {
                        if *crop_id == id && *handle == handle_type {
                            if let Some(current_pos) = input.interact_pos {
                                let screen_delta = current_pos - *start_mouse;
                                let delta = if *screen_space {
                                    screen_delta
                                } else {
                                    screen_delta / self.zoom
                                };
                                let (new_crop, new_rect) =
                                    apply_crop(*handle, *start_crop_uv, *start_rect, delta);

                                // Apply the new crop
                                if let Some(preview) = preview_manager.get_mut(id) {
                                    preview.crop_uv = Some(new_crop);
                                    if *screen_space {
                                        preview.viewport_pin =
                                            Some(ViewportPin::from_rect(new_rect, canvas_rect));
                                    } else {
                                        preview.position = new_rect.min;
                                        preview.size = new_rect.size();
                                    }
                                    // Update aspect ratio based on new crop region
                                    if let Some((w, h)) = frame_size {
                                        let crop_width = (new_crop.2 - new_crop.0) * w as f32;
                                        let crop_height = (new_crop.3 - new_crop.1) * h as f32;
                                        if crop_height > 0.0 {
                                            preview.source_aspect_ratio = crop_width / crop_height;
                                        }
                                    } else if new_rect.height() > 0.0 {
                                        preview.source_aspect_ratio =
                                            new_rect.width() / new_rect.height();
                                    }
                                }
                            }
                        }
                    }
                }

                // Clear drag state on release
                if handle_response.drag_stopped() {
                    if let Some(DragState::Resizing {
                        id: resize_id,
                        handle,
                        ..
                    }) = &self.drag_state
                    {
                        if *resize_id == id && *handle == handle_type {
                            self.drag_state = None;
                        }
                    }
                    if let Some(DragState::Cropping {
                        id: crop_id,
                        handle,
                        ..
                    }) = &self.drag_state
                    {
                        if *crop_id == id && *handle == handle_type {
                            self.drag_state = None;
                        }
                    }
                }
            }
        }
    }
}

/// Adjust crop UVs and the tile rectangle together. Keeping one coordinate unit
/// per source UV unit prevents the cropped image from stretching and also keeps
/// the capture backing target stable while a handle moves.
fn apply_crop(
    handle: ResizeHandle,
    start_crop: (f32, f32, f32, f32),
    start_rect: Rect,
    delta: Vec2,
) -> ((f32, f32, f32, f32), Rect) {
    let (min_u, min_v, max_u, max_v) = start_crop;
    let u_span = (max_u - min_u).max(0.001);
    let v_span = (max_v - min_v).max(0.001);
    let uv_delta_x = delta.x / start_rect.width().max(0.001) * u_span;
    let uv_delta_y = delta.y / start_rect.height().max(0.001) * v_span;
    let mut crop = start_crop;

    match handle {
        ResizeHandle::TopLeft => {
            crop.0 = (min_u + uv_delta_x).clamp(0.0, max_u - 0.1);
            crop.1 = (min_v + uv_delta_y).clamp(0.0, max_v - 0.1);
        }
        ResizeHandle::Top => {
            crop.1 = (min_v + uv_delta_y).clamp(0.0, max_v - 0.1);
        }
        ResizeHandle::TopRight => {
            crop.2 = (max_u + uv_delta_x).clamp(min_u + 0.1, 1.0);
            crop.1 = (min_v + uv_delta_y).clamp(0.0, max_v - 0.1);
        }
        ResizeHandle::Left => {
            crop.0 = (min_u + uv_delta_x).clamp(0.0, max_u - 0.1);
        }
        ResizeHandle::Right => {
            crop.2 = (max_u + uv_delta_x).clamp(min_u + 0.1, 1.0);
        }
        ResizeHandle::BottomLeft => {
            crop.0 = (min_u + uv_delta_x).clamp(0.0, max_u - 0.1);
            crop.3 = (max_v + uv_delta_y).clamp(min_v + 0.1, 1.0);
        }
        ResizeHandle::Bottom => {
            crop.3 = (max_v + uv_delta_y).clamp(min_v + 0.1, 1.0);
        }
        ResizeHandle::BottomRight => {
            crop.2 = (max_u + uv_delta_x).clamp(min_u + 0.1, 1.0);
            crop.3 = (max_v + uv_delta_y).clamp(min_v + 0.1, 1.0);
        }
    }

    (crop, rect_for_crop_change(start_crop, crop, start_rect))
}

fn rect_for_crop_change(
    start_crop: (f32, f32, f32, f32),
    new_crop: (f32, f32, f32, f32),
    start_rect: Rect,
) -> Rect {
    let (min_u, min_v, max_u, max_v) = start_crop;
    let units_per_u = start_rect.width() / (max_u - min_u).max(0.001);
    let units_per_v = start_rect.height() / (max_v - min_v).max(0.001);
    Rect::from_min_max(
        Pos2::new(
            start_rect.min.x + (new_crop.0 - min_u) * units_per_u,
            start_rect.min.y + (new_crop.1 - min_v) * units_per_v,
        ),
        Pos2::new(
            start_rect.max.x + (new_crop.2 - max_u) * units_per_u,
            start_rect.max.y + (new_crop.3 - max_v) * units_per_v,
        ),
    )
}

/// Apply resize delta based on handle position, optionally maintaining aspect ratio
fn apply_resize(
    handle: ResizeHandle,
    start_rect: Rect,
    delta: Vec2,
    aspect_ratio: Option<f32>,
) -> Rect {
    let mut min = start_rect.min;
    let mut max = start_rect.max;

    match handle {
        ResizeHandle::TopLeft => {
            min.x += delta.x;
            min.y += delta.y;
        }
        ResizeHandle::Top => {
            min.y += delta.y;
        }
        ResizeHandle::TopRight => {
            max.x += delta.x;
            min.y += delta.y;
        }
        ResizeHandle::Left => {
            min.x += delta.x;
        }
        ResizeHandle::Right => {
            max.x += delta.x;
        }
        ResizeHandle::BottomLeft => {
            min.x += delta.x;
            max.y += delta.y;
        }
        ResizeHandle::Bottom => {
            max.y += delta.y;
        }
        ResizeHandle::BottomRight => {
            max.x += delta.x;
            max.y += delta.y;
        }
    }

    // Ensure min < max
    if min.x > max.x {
        std::mem::swap(&mut min.x, &mut max.x);
    }
    if min.y > max.y {
        std::mem::swap(&mut min.y, &mut max.y);
    }

    let mut result = Rect::from_min_max(min, max);

    // Apply aspect ratio constraint if provided
    if let Some(ar) = aspect_ratio {
        let width = result.width();
        let height = result.height();
        let current_ar = width / height;

        // Determine which dimension to adjust based on handle and direction
        match handle {
            // Corner handles - use the dominant movement direction
            ResizeHandle::TopLeft
            | ResizeHandle::TopRight
            | ResizeHandle::BottomLeft
            | ResizeHandle::BottomRight => {
                if current_ar > ar {
                    // Too wide - adjust width to match height
                    let new_width = height * ar;
                    match handle {
                        ResizeHandle::TopLeft | ResizeHandle::BottomLeft => {
                            result = Rect::from_min_size(
                                Pos2::new(result.max.x - new_width, result.min.y),
                                Vec2::new(new_width, height),
                            );
                        }
                        _ => {
                            result = Rect::from_min_size(result.min, Vec2::new(new_width, height));
                        }
                    }
                } else {
                    // Too tall - adjust height to match width
                    let new_height = width / ar;
                    match handle {
                        ResizeHandle::TopLeft | ResizeHandle::TopRight => {
                            result = Rect::from_min_size(
                                Pos2::new(result.min.x, result.max.y - new_height),
                                Vec2::new(width, new_height),
                            );
                        }
                        _ => {
                            result = Rect::from_min_size(result.min, Vec2::new(width, new_height));
                        }
                    }
                }
            }
            // Edge handles - adjust the perpendicular dimension
            ResizeHandle::Top | ResizeHandle::Bottom => {
                let new_width = height * ar;
                let center_x = result.center().x;
                result = Rect::from_center_size(
                    Pos2::new(center_x, result.center().y),
                    Vec2::new(new_width, height),
                );
            }
            ResizeHandle::Left | ResizeHandle::Right => {
                let new_height = width / ar;
                let center_y = result.center().y;
                result = Rect::from_center_size(
                    Pos2::new(result.center().x, center_y),
                    Vec2::new(width, new_height),
                );
            }
        }
    }

    result
}
