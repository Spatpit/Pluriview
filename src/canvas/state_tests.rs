use super::{
    apply_crop, apply_resize, browser_control_colors, capture_resolution_badge_rect, format_time,
    live_capture_display_size, media_placeholder_content, native_capture_canvas_size,
    pixel_aligned_rect, playlist_first_row_center, rect_for_crop_change, stream_audio_badge_rect,
    submenu_should_open_left, video_placeholder_content, video_time_style,
    video_volume_popover_rect, video_volume_slider_rect, window_capture_placeholder_content,
    window_capture_target, BrowserAction, CanvasState, DragState, PlaylistAction, ResizeHandle,
    TileActivityAction, VideoAction,
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
    let (idle_background, idle_icon) = browser_control_colors(BrowserAction::Back, false, false);
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
    let id = previews.add_for_window(123, 42, "editor".to_owned(), Pos2::ZERO, Vec2::splat(100.0));
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
fn long_tile_menu_scrolls_to_its_final_action_in_a_short_viewport() {
    let ctx = Context::default();
    crate::ui_theme::install(&ctx);
    let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));
    let pointer = Pos2::new(300.0, 100.0);
    let mut frame = 0;
    let mut render = |events| {
        frame += 1;
        ctx.run(
            RawInput {
                screen_rect: Some(screen),
                time: Some(frame as f64 / 60.0),
                events,
                ..Default::default()
            },
            |ctx| {
                CentralPanel::default().show(ctx, |ui| {
                    let response = ui.allocate_rect(ui.max_rect(), Sense::click());
                    super::show_context_menu_if(&response, true, |ui| {
                        for index in 0..40 {
                            let _ = ui.button(format!("Action {index}"));
                        }
                    });
                });
            },
        )
    };
    render(vec![Event::PointerMoved(pointer)]);
    for pressed in [true, false] {
        render(vec![Event::PointerButton {
            pos: pointer,
            button: PointerButton::Secondary,
            pressed,
            modifiers: Modifiers::NONE,
        }]);
    }
    let last_visible = |output: &egui::FullOutput| {
        output.shapes.iter().any(|shape| {
        matches!(&shape.shape, Shape::Text(text) if text.galley.text() == "Action 39"
            && shape.clip_rect.intersect(screen).contains_rect(Rect::from_min_size(text.pos, text.galley.size())))
    })
    };
    assert!(!last_visible(&render(vec![])));
    let mut reached_last = false;
    for _ in 0..20 {
        let output = render(vec![
            Event::PointerMoved(pointer + Vec2::new(20.0, 10.0)),
            Event::MouseWheel {
                unit: MouseWheelUnit::Point,
                delta: Vec2::new(0.0, -150.0),
                modifiers: Modifiers::NONE,
            },
        ]);
        reached_last |= last_visible(&output);
    }
    assert!(
        reached_last,
        "Scrolling should expose the last action inside the viewport"
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
    let window = previews.add_for_window(1, 42, "game".to_owned(), Pos2::ZERO, Vec2::splat(100.0));
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
    let mut canvas = CanvasState {
        drag_state: Some(DragState::Resizing {
            id: PreviewId(1),
            handle: ResizeHandle::BottomRight,
            start_rect: Rect::from_min_size(Pos2::ZERO, Vec2::splat(100.0)),
            start_mouse: Pos2::new(100.0, 100.0),
            aspect_ratio: Some(1.0),
            screen_space: false,
        }),
        ..Default::default()
    };
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

struct CropUiTest {
    context: Context,
    canvas: CanvasState,
    previews: PreviewManager,
    captures: CaptureCoordinator,
    viewport: Rect,
    time: f64,
}

impl CropUiTest {
    fn new(kind: u8, pinned: bool) -> (Self, PreviewId) {
        let mut previews = PreviewManager::new();
        let position = Pos2::new(100.0, 100.0);
        let size = Vec2::new(200.0, 100.0);
        let id = match kind {
            0 => previews.add_for_window(1, 42, "window".into(), position, size),
            1 => previews.add_browser_placeholder(
                "https://example.com".into(),
                position,
                size,
                FpsPreset::Medium,
            ),
            2 | 3 => previews.add_media(
                PathBuf::from("fixture.png"),
                "image".into(),
                (0..if kind == 3 { 2 } else { 1 })
                    .map(|_| crate::media::MediaFrame {
                        width: 200,
                        height: 100,
                        rgba: vec![0; 200 * 100 * 4],
                        duration: Duration::from_millis(100),
                    })
                    .collect(),
                position,
                size,
            ),
            4 => previews.add_video_placeholder(
                VideoSource::LocalFile {
                    path: PathBuf::from("fixture.mp4"),
                },
                "video".into(),
                position,
                size,
                FpsPreset::Medium,
                false,
            ),
            5 => previews.add_for_spout("fixture".into(), position, size, FpsPreset::Medium),
            _ => unreachable!(),
        };
        if matches!(kind, 0 | 1 | 5) {
            previews
                .get_mut(id)
                .unwrap()
                .update_frame(200, 100, vec![0; 200 * 100 * 4]);
        }
        let viewport = Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 750.0));
        if pinned {
            previews.get_mut(id).unwrap().viewport_pin =
                Some(crate::preview::ViewportPin::from_rect(
                    Rect::from_min_size(position, size),
                    viewport,
                ));
        }
        (
            Self {
                context: Context::default(),
                canvas: CanvasState {
                    pan: Vec2::new(17.0, -9.0),
                    zoom: 1.25,
                    ..Default::default()
                },
                previews,
                captures: CaptureCoordinator::new(),
                viewport,
                time: 0.0,
            },
            id,
        )
    }

    fn frame(&mut self, events: Vec<Event>) -> egui::FullOutput {
        self.time += 0.1;
        self.context.run(
            RawInput {
                screen_rect: Some(self.viewport),
                time: Some(self.time),
                events,
                ..Default::default()
            },
            |context| {
                CentralPanel::default()
                    .frame(egui::Frame::none())
                    .show(context, |ui| {
                        self.canvas
                            .ui(ui, &mut self.previews, &mut self.captures, context, true);
                    });
            },
        )
    }

    fn click(&mut self, pos: Pos2, button: PointerButton) -> egui::FullOutput {
        self.frame(vec![Event::PointerMoved(pos)]);
        for pressed in [true, false] {
            self.frame(vec![Event::PointerButton {
                pos,
                button,
                pressed,
                modifiers: Modifiers::NONE,
            }]);
        }
        self.frame(vec![])
    }

    fn label_pos(output: &egui::FullOutput, label: &str) -> Pos2 {
        output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                Shape::Text(text) if text.galley.text() == label => {
                    Some(text.pos + text.galley.rect.center().to_vec2())
                }
                _ => None,
            })
            .unwrap_or_else(|| {
                panic!(
                    "Missing menu label: {label}; labels: {:?}",
                    output
                        .shapes
                        .iter()
                        .filter_map(|s| match &s.shape {
                            Shape::Text(t) => Some(t.galley.text()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                )
            })
    }

    fn open_crop_menu(&mut self, id: PreviewId) -> egui::FullOutput {
        let rect = self
            .canvas
            .preview_screen_rect(self.previews.get(id).unwrap(), self.viewport);
        let mut output = self.click(rect.center(), PointerButton::Secondary);
        for _ in 0..12 {
            if output.shapes.iter().any(|shape| {
                matches!(&shape.shape,
                Shape::Text(text) if text.galley.text() == "Crop"
                    && shape.clip_rect.contains(text.pos + text.galley.rect.center().to_vec2()))
            }) {
                break;
            }
            self.frame(vec![
                Event::PointerMoved(rect.center() + Vec2::new(20.0, 10.0)),
                Event::MouseWheel {
                    unit: MouseWheelUnit::Point,
                    delta: Vec2::new(0.0, -150.0),
                    modifiers: Modifiers::NONE,
                },
            ]);
            output = self.frame(vec![]);
        }
        self.click(Self::label_pos(&output, "Crop"), PointerButton::Primary)
    }

    fn drag(&mut self, start: Pos2, end: Pos2) {
        self.frame(vec![Event::PointerMoved(start)]);
        self.frame(vec![Event::PointerButton {
            pos: start,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        }]);
        self.frame(vec![Event::PointerMoved(end)]);
        self.frame(vec![Event::PointerButton {
            pos: end,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }]);
    }
}

#[test]
fn menu_crop_handles_work_for_all_sources_and_pinned_tiles() {
    for kind in 0..6 {
        for pinned in [false, true] {
            let (mut test, id) = CropUiTest::new(kind, pinned);
            test.frame(vec![]);
            let output = test.open_crop_menu(id);
            test.click(
                CropUiTest::label_pos(&output, "Edit Crop"),
                PointerButton::Primary,
            );
            assert_eq!(
                test.canvas.crop_mode,
                Some(id),
                "source {kind}, pinned {pinned}"
            );
            let pan = test.canvas.pan;
            let before = test
                .canvas
                .preview_screen_rect(test.previews.get(id).unwrap(), test.viewport);
            let start = before.right_center();
            test.drag(start, start - Vec2::new(before.width() * 0.25, 0.0));
            let preview = test.previews.get(id).unwrap();
            let crop = preview.crop_uv.expect("Menu handle must crop without Alt");
            assert!(
                (crop.2 - 0.75).abs() < 0.01,
                "source {kind}, pinned {pinned}: {crop:?}"
            );
            let after = test.canvas.preview_screen_rect(preview, test.viewport);
            assert!((after.width() / before.width() - 0.75).abs() < 0.01);
            assert!((after.height() - before.height()).abs() < 0.01);
            assert_eq!(test.canvas.pan, pan);
            assert_eq!(
                test.canvas.crop_mode,
                Some(id),
                "Mode must survive a handle release"
            );
            assert!(test.canvas.last_double_clicked.is_none());

            test.drag(after.left_top(), after.left_top() + before.size() * 0.1);
            let crop = test.previews.get(id).unwrap().crop_uv.unwrap();
            assert!((crop.0 - 0.1).abs() < 0.01 && (crop.1 - 0.1).abs() < 0.01);
            assert!(
                (crop.2 - 0.75).abs() < 0.01,
                "Repeated corner crop must keep the previous edge"
            );
            let output = test.open_crop_menu(id);
            test.click(
                CropUiTest::label_pos(&output, "Clear Crop"),
                PointerButton::Primary,
            );
            let restored = test
                .canvas
                .preview_screen_rect(test.previews.get(id).unwrap(), test.viewport);
            assert!((restored.width() - before.width()).abs() < 0.1);
            assert!(test.previews.get(id).unwrap().crop_uv.is_none());
            let output = test.open_crop_menu(id);
            test.click(
                CropUiTest::label_pos(&output, "Finish Cropping"),
                PointerButton::Primary,
            );
            assert!(test.canvas.crop_mode.is_none());
        }
    }
}

#[test]
fn empty_canvas_menu_queues_app_topmost_setting() {
    let (mut test, _) = CropUiTest::new(2, false);
    test.frame(vec![]);
    let output = test.click(Pos2::new(500.0, 50.0), PointerButton::Secondary);
    test.click(
        CropUiTest::label_pos(&output, "Always on Top"),
        PointerButton::Primary,
    );
    assert_eq!(test.canvas.pending_always_on_top.take(), Some(true));
    let output = test.click(Pos2::new(500.0, 50.0), PointerButton::Secondary);
    test.click(
        CropUiTest::label_pos(&output, "Always on Top"),
        PointerButton::Primary,
    );
    assert_eq!(test.canvas.pending_always_on_top.take(), Some(false));
}

#[test]
fn passing_input_through_cancels_crop_without_changing_geometry() {
    let (mut test, id) = CropUiTest::new(1, true);
    test.canvas.begin_crop(id, &test.previews);
    let before = crate::preview::PreviewLayout::from(test.previews.get(id).unwrap());
    test.canvas.interactive_browser = Some(id);
    test.canvas.last_double_clicked = Some(id);
    test.canvas.cancel_pointer_interaction();
    assert!(test.canvas.crop_mode.is_none());
    assert!(test.canvas.interactive_browser.is_none());
    assert!(test.canvas.last_double_clicked.is_none());
    assert_eq!(
        serde_json::to_value(crate::preview::PreviewLayout::from(
            test.previews.get(id).unwrap()
        ))
        .unwrap(),
        serde_json::to_value(before).unwrap()
    );
}

#[test]
fn crop_mode_keeps_tile_still_and_empty_canvas_restores_normal_resize() {
    for pinned in [false, true] {
        let (mut test, id) = CropUiTest::new(2, pinned);
        test.canvas.begin_crop(id, &test.previews);
        test.frame(vec![]);
        let before = test
            .canvas
            .preview_screen_rect(test.previews.get(id).unwrap(), test.viewport);
        test.drag(before.center(), before.center() + Vec2::new(30.0, 15.0));
        assert_eq!(
            test.canvas
                .preview_screen_rect(test.previews.get(id).unwrap(), test.viewport),
            before
        );
        test.click(Pos2::new(800.0, 650.0), PointerButton::Primary);
        assert!(test.canvas.crop_mode.is_none());
        test.click(before.center(), PointerButton::Primary);
        test.drag(
            before.right_center(),
            before.right_center() + Vec2::new(50.0, 0.0),
        );
        let preview = test.previews.get(id).unwrap();
        assert!(
            preview.crop_uv.is_none(),
            "Ordinary resize must resume after crop mode"
        );
        assert!(
            test.canvas
                .preview_screen_rect(preview, test.viewport)
                .width()
                > before.width()
        );
    }
}

#[test]
fn crop_mode_exits_on_escape_removal_workspace_change_and_focus() {
    let (mut test, id) = CropUiTest::new(2, false);
    test.canvas.begin_crop(id, &test.previews);
    test.frame(vec![]);
    test.frame(vec![Event::Key {
        key: egui::Key::Escape,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    }]);
    assert!(test.canvas.crop_mode.is_none());
    test.canvas.begin_crop(id, &test.previews);
    test.canvas.clear_preview_interactions();
    assert!(test.canvas.crop_mode.is_none());
    test.canvas.begin_crop(id, &test.previews);
    test.canvas
        .focus_on_tile(id, test.previews.get(id).unwrap().rect(), test.viewport);
    assert!(test.canvas.crop_mode.is_none());
    test.canvas.begin_crop(id, &test.previews);
    test.canvas.selection.clear();
    test.canvas.prune_preview_interactions(&test.previews);
    assert!(test.canvas.crop_mode.is_none());
    test.canvas.begin_crop(id, &test.previews);
    test.previews.start_removal(id);
    test.canvas.prune_preview_interactions(&test.previews);
    assert!(test.canvas.crop_mode.is_none());
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
    let mut canvas = CanvasState {
        zoom: 2.0,
        ..Default::default()
    };
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
    let mut canvas = CanvasState {
        pan: Vec2::new(21.0, -8.0),
        zoom: 0.75,
        selection: vec![PreviewId(9)],
        ..Default::default()
    };
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
fn viewer_focus_uses_topmost_tile_toggles_restore_and_ignores_pinned_tiles() {
    let mut canvas = CanvasState::default();
    let viewport = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));
    canvas.last_screen_rect = Some(viewport);
    let mut previews = PreviewManager::new();
    let lower = previews.add("lower".into(), Pos2::new(50.0, 50.0), Vec2::splat(100.0));
    let upper = previews.add("upper".into(), Pos2::new(50.0, 50.0), Vec2::splat(100.0));
    canvas.selection = vec![lower];
    let geometry = previews.get(upper).unwrap().rect();
    canvas.focus_viewer_at([100.0 / 800.0, 100.0 / 600.0], &previews);
    assert!(canvas.is_focusing_tile());
    assert_eq!(canvas.selection, vec![upper]);
    assert_eq!(previews.get(upper).unwrap().rect(), geometry);
    assert_eq!(
        canvas.last_double_clicked, None,
        "Never activate source content"
    );
    canvas.focus_viewer_at([0.5, 0.5], &previews);
    assert!(!canvas.is_focusing_tile());
    assert_eq!(canvas.selection, vec![lower]);
    assert_eq!(canvas.pan, Vec2::ZERO);
    assert_eq!(canvas.zoom, 1.0);
    canvas.focus_viewer_at([0.95, 0.95], &previews);
    assert!(!canvas.is_focusing_tile());
    canvas.toggle_viewport_pin(upper, viewport, &mut previews);
    canvas.focus_viewer_at([100.0 / 800.0, 100.0 / 600.0], &previews);
    assert!(
        !canvas.is_focusing_tile(),
        "A pinned tile must not focus itself or a tile behind it"
    );
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
    let mut canvas = CanvasState {
        selection: vec![id, PreviewId(99)],
        ..Default::default()
    };
    let viewport = Rect::from_min_size(Pos2::ZERO, Vec2::splat(500.0));
    canvas.focus_on_tile(id, previews.get(id).unwrap().rect(), viewport);

    previews.start_removal(id);
    canvas.refit_focus(&previews, viewport);

    assert_eq!(canvas.selection, vec![PreviewId(99)]);
    assert!(!canvas.exit_focus());
}

#[test]
fn shift_wheel_over_a_browser_tile_scrolls_its_page_instead_of_zooming() {
    let context = Context::default();
    let mut canvas = CanvasState::default();
    let mut previews = PreviewManager::new();
    let id = previews.add_browser_placeholder(
        "https://example.com".to_owned(),
        Pos2::new(100.0, 100.0),
        Vec2::new(200.0, 100.0),
        FpsPreset::Medium,
    );
    let preview = previews.get_mut(id).unwrap();
    preview.browser_status = crate::preview::BrowserTileStatus::Ready;
    preview.crop_uv = Some((0.5, 0.0, 1.0, 1.0));
    let mut captures = CaptureCoordinator::new();
    let screen_rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(700.0, 500.0));
    let tile = canvas.canvas_rect_to_screen(previews.get(id).unwrap().rect(), screen_rect);
    let pointer = tile.center();
    let wheel = |canvas: &mut CanvasState,
                 previews: &mut PreviewManager,
                 captures: &mut CaptureCoordinator,
                 modifiers: Modifiers| {
        let _ = context.run(
            RawInput {
                screen_rect: Some(screen_rect),
                events: vec![
                    Event::PointerMoved(pointer),
                    Event::MouseWheel {
                        unit: MouseWheelUnit::Point,
                        delta: Vec2::new(0.0, -50.0),
                        modifiers,
                    },
                ],
                modifiers,
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
    };

    let zoom = canvas.zoom;
    wheel(&mut canvas, &mut previews, &mut captures, Modifiers::SHIFT);
    assert_eq!(canvas.zoom, zoom);
    let scrolls = std::mem::take(&mut canvas.pending_browser_scrolls);
    assert_eq!(scrolls.len(), 1);
    let (scrolled, u, v, delta) = scrolls[0];
    assert_eq!(scrolled, id);
    // The tile shows the right half of the page, so its centre is at 75%.
    assert!((u - 0.75).abs() < 0.01 && (v - 0.5).abs() < 0.01);
    assert!(
        delta > 0.0,
        "scrolling the wheel down scrolls the page down"
    );

    wheel(&mut canvas, &mut previews, &mut captures, Modifiers::NONE);
    assert!(canvas.pending_browser_scrolls.is_empty());
    assert_ne!(canvas.zoom, zoom);
}
