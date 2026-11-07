use super::*;
use crate::preview::PreviewLayout;
use GroupArrangement::{Equal, Free, Main};

const ARRANGEMENTS: [GroupArrangement; 4] = [Main(1), Main(2), Main(3), Equal];
const TILE_ASPECT: f32 = 300.0 / 180.0;

fn fixture() -> (CanvasState, PreviewManager, Vec<PreviewId>, Rect) {
    let mut canvas = CanvasState {
        pan: Vec2::new(25.0, -10.0),
        zoom: 0.8,
        ..Default::default()
    };
    let viewport = Rect::from_min_size(Pos2::new(200.0, 40.0), Vec2::new(1200.0, 800.0));
    canvas.last_screen_rect = Some(viewport);
    let mut previews = PreviewManager::new();
    let ids: Vec<_> = (0..4)
        .map(|i| {
            previews.add(
                format!("Tile {i}"),
                Pos2::new((i % 2) as f32 * 320.0, (i / 2) as f32 * 200.0),
                Vec2::new(300.0, 180.0),
            )
        })
        .collect();
    canvas.selection = ids.clone();
    canvas.create_group(viewport, &mut previews);
    (canvas, previews, ids, viewport)
}

/// Tiles squashed into the same 300×180 shape, showing sources of very
/// different shapes: a phone-shaped window, a wide window, a square crop of a
/// 4:3 capture and a landscape image.
fn stretched_fixture() -> (CanvasState, PreviewManager, Vec<PreviewId>, Rect, [f32; 4]) {
    let (canvas, mut previews, ids, viewport) = fixture();
    let sources = [
        (Some((540, 1170)), None, None),
        (Some((2560, 1080)), None, None),
        (Some((1600, 1200)), None, Some((0.125, 0.0, 0.875, 1.0))),
        (None, Some((1200, 800)), None),
    ];
    for (id, (source, frame, crop)) in ids.iter().zip(sources) {
        let p = previews.get_mut(*id).unwrap();
        p.source_frame_size = source;
        p.frame_size = frame;
        p.crop_uv = crop;
    }
    let aspects = [540.0 / 1170.0, 2560.0 / 1080.0, 1.0, 1.5];
    (canvas, previews, ids, viewport, aspects)
}

fn layout_of(previews: &PreviewManager, ids: &[PreviewId], viewport: Rect) -> Vec<Rect> {
    ids.iter()
        .map(|id| layout_rect(previews.get(*id).unwrap(), viewport))
        .collect()
}

fn screen_of(
    canvas: &CanvasState,
    previews: &PreviewManager,
    ids: &[PreviewId],
    viewport: Rect,
) -> Vec<Rect> {
    ids.iter()
        .map(|id| canvas.target_screen_rect(previews.get(*id).unwrap(), viewport))
        .collect()
}

fn box_of(canvas: &CanvasState, previews: &PreviewManager, id: PreviewId, viewport: Rect) -> Rect {
    let (outline, arranged) = canvas.group_outline(id, viewport, previews).unwrap();
    assert!(arranged);
    outline
}

fn arrangement_of(previews: &PreviewManager, id: PreviewId) -> GroupArrangement {
    previews
        .get(id)
        .unwrap()
        .tile_group
        .as_ref()
        .unwrap()
        .arrangement
}

fn assert_rects(actual: &[Rect], expected: &[Rect], tolerance: f32, context: &str) {
    assert_eq!(actual.len(), expected.len(), "{context}");
    for (index, (a, b)) in actual.iter().zip(expected).enumerate() {
        assert!(
            rects_close(*a, *b, tolerance),
            "{context}: tile {index} is {a:?}, expected {b:?}"
        );
    }
}

/// Every tile keeps its shape, stays inside `inside` and overlaps no other.
fn assert_tidy(rects: &[Rect], aspects: &[f32], inside: Rect, context: &str) {
    for (index, (rect, aspect)) in rects.iter().zip(aspects).enumerate() {
        assert!(rect.is_positive(), "{context}: tile {index} {rect:?}");
        assert!(
            inside.expand(0.05).contains_rect(*rect),
            "{context}: tile {index} {rect:?} outside {inside:?}"
        );
        assert!(
            (rect.aspect_ratio() / aspect - 1.0).abs() < 0.001,
            "{context}: tile {index} is {} instead of {aspect}",
            rect.aspect_ratio()
        );
        for other in &rects[..index] {
            assert!(
                !rect.shrink(0.01).intersects(*other),
                "{context}: tile {index} overlaps"
            );
        }
    }
}

fn rows(rects: &[Rect]) -> usize {
    let mut centers: Vec<f32> = rects.iter().map(|r| r.center().y).collect();
    centers.sort_by(f32::total_cmp);
    centers.dedup_by(|a, b| (*a - *b).abs() < 0.5);
    centers.len()
}

/// Run the per-frame layout pass until it has nothing left to do.
fn settle(canvas: &mut CanvasState, previews: &mut PreviewManager, viewport: Rect, now: f64) {
    for _ in 0..3 {
        canvas.update_group_layouts(viewport, previews, now);
    }
}

#[test]
fn layouts_keep_shapes_inside_the_box_without_overlap() {
    let aspect_sets: [&[f32]; 4] = [
        &[16.0 / 9.0, 0.5, 1.0, 2.4, 4.0 / 3.0],
        &[0.25, 4.0, 1.0, 0.5, 2.4, 1.5, 0.75],
        &[16.0 / 9.0; 2],
        &[0.5625; 9],
    ];
    let frames = [
        Rect::from_min_size(Pos2::new(30.0, -15.0), Vec2::new(1200.0, 700.0)),
        Rect::from_min_size(Pos2::new(-400.0, 50.0), Vec2::new(350.0, 900.0)),
        Rect::from_min_size(Pos2::new(5.0, 5.0), Vec2::new(2600.0, 300.0)),
        Rect::from_min_size(Pos2::ZERO, Vec2::splat(10.0)),
    ];
    for aspects in aspect_sets {
        for frame in frames {
            for arrangement in ARRANGEMENTS {
                for gap in [GroupGap::None, GroupGap::Small, GroupGap::Large] {
                    let rects = layout_rects(aspects, frame, arrangement, gap);
                    assert_tidy(
                        &rects,
                        aspects,
                        frame,
                        &format!("{arrangement:?} {gap:?} {frame:?}"),
                    );
                }
            }
        }
    }
    assert!(layout_rects(&[], Rect::EVERYTHING, Equal, GroupGap::Small).is_empty());
}

#[test]
fn layouts_scale_and_move_with_their_box() {
    // Moving or scaling a whole group keeps its box relative to every member,
    // so the layout pass finds nothing to change.
    let aspects = [0.5, 16.0 / 9.0, 1.0, 2.4, 0.75, 1.5];
    let frame = Rect::from_min_size(Pos2::new(30.0, 50.0), Vec2::new(1200.0, 700.0));
    for arrangement in ARRANGEMENTS {
        let rects = layout_rects(&aspects, frame, arrangement, GroupGap::Small);
        let (scale, shift) = (1.7, Vec2::new(123.0, -45.0));
        let moved = Rect::from_min_size(frame.min * scale + shift, frame.size() * scale);
        let expected: Vec<_> = rects
            .iter()
            .map(|r| Rect::from_min_size(r.min * scale + shift, r.size() * scale))
            .collect();
        assert_rects(
            &layout_rects(&aspects, moved, arrangement, GroupGap::Small),
            &expected,
            frame_tolerance(moved),
            &format!("{arrangement:?}"),
        );
    }
}

#[test]
fn equal_tiles_follow_the_box_and_center_short_rows() {
    let aspects = [16.0 / 9.0; 4];
    let wide = Rect::from_min_size(Pos2::ZERO, Vec2::new(1600.0, 230.0));
    assert_eq!(
        rows(&layout_rects(&aspects, wide, Equal, GroupGap::Small)),
        1
    );
    let tall = Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 1000.0));
    assert_eq!(
        rows(&layout_rects(&aspects, tall, Equal, GroupGap::Small)),
        4
    );
    let even = Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 720.0));
    assert_eq!(
        rows(&layout_rects(&aspects, even, Equal, GroupGap::Small)),
        2
    );
    // Reapplying to its own outline keeps the same arrangement. (The gap
    // follows the box's size, so a tighter box nudges tiles by a fraction.)
    for frame in [wide, tall, even] {
        let first = layout_rects(&aspects, frame, Equal, GroupGap::Small);
        let outline = bounds_of(first.iter().copied()).unwrap();
        let again = layout_rects(&aspects, outline, Equal, GroupGap::Small);
        assert_rects(&again, &first, 0.5, "reapplied");
    }
    // Three tiles in two rows: the lone tile sits in the middle.
    let rects = layout_rects(&[16.0 / 9.0; 3], even, Equal, GroupGap::Small);
    assert_eq!(rows(&rects), 2);
    let content = bounds_of(rects.iter().copied()).unwrap();
    assert!((rects[2].center().x - content.center().x).abs() < 0.01);
    assert!((content.center() - even.center()).length() < 0.01);
    // Mixed shapes share a row's middle line.
    let rects = layout_rects(&[0.5, 16.0 / 9.0], wide, Equal, GroupGap::Small);
    assert!((rects[0].center().y - rects[1].center().y).abs() < 0.01);
}

#[test]
fn large_tiles_share_a_row_and_companions_stay_readable() {
    let frame = Rect::from_min_size(Pos2::ZERO, Vec2::new(1600.0, 900.0));
    for count in 2..=10 {
        for large in 1..=3_u8 {
            if large as usize >= count {
                continue;
            }
            let aspects = vec![16.0 / 9.0; count];
            let rects = layout_rects(&aspects, frame, Main(large), GroupGap::Small);
            let big = &rects[..large as usize];
            for tile in big {
                assert!((tile.height() - big[0].height()).abs() < 0.01);
                assert!((tile.top() - big[0].top()).abs() < 0.01);
            }
            for tile in &rects[large as usize..] {
                let scale = (tile.area() / big[0].area()).sqrt();
                assert!(
                    scale >= COMPANION_MIN_SCALE - 0.001
                        && scale <= COMPANION_MAX_AREA.sqrt() + 0.001,
                    "{count} tiles, {large} large: companion scale {scale}"
                );
            }
        }
    }
}

#[test]
fn companions_go_beside_in_a_wide_box_and_below_in_a_tall_one() {
    let aspects = [16.0 / 9.0; 4];
    let wide = Rect::from_min_size(Pos2::ZERO, Vec2::new(2400.0, 600.0));
    let rects = layout_rects(&aspects, wide, Main(1), GroupGap::Small);
    assert!(rects[1..].iter().all(|r| r.left() > rects[0].right()));
    let tall = Rect::from_min_size(Pos2::ZERO, Vec2::new(600.0, 1400.0));
    let rects = layout_rects(&aspects, tall, Main(1), GroupGap::Small);
    assert!(rects[1..].iter().all(|r| r.top() > rects[0].bottom()));
}

#[test]
fn gap_setting_sets_the_space_between_tiles() {
    let frame = Rect::from_min_size(Pos2::ZERO, Vec2::new(1600.0, 450.0));
    let space = |gap| {
        let rects = layout_rects(&[16.0 / 9.0; 2], frame, Equal, gap);
        rects[1].left() - rects[0].right()
    };
    assert!(space(GroupGap::None).abs() < 0.001);
    assert!(space(GroupGap::Small) > 1.0);
    assert!(space(GroupGap::Large) > space(GroupGap::Small) * 2.0);
}

#[test]
fn arranging_a_free_group_keeps_its_area_and_corner() {
    for pinned in [false, true] {
        let (mut canvas, mut previews, ids, viewport) = fixture();
        if pinned {
            canvas.toggle_viewport_pin(ids[0], viewport, &mut previews);
        }
        let before = layout_of(&previews, &ids, viewport);
        let area: f32 = before.iter().map(Rect::area).sum();
        let corner = bounds_of(before.iter().copied()).unwrap().min;
        canvas.set_group_arrangement(ids[0], Equal, None, viewport, &mut previews);
        let after = layout_of(&previews, &ids, viewport);
        let arranged: f32 = after.iter().map(Rect::area).sum();
        assert!((arranged / area - 1.0).abs() < 0.05, "{arranged} vs {area}");
        let frame = canvas.to_layout_space(
            box_of(&canvas, &previews, ids[0], viewport),
            pinned,
            viewport,
        );
        assert!(frame.min.distance(corner) < 0.05);
        assert_tidy(&after, &[TILE_ASPECT; 4], frame, "equal");
    }
}

#[test]
fn switching_arrangements_keeps_the_box_and_switching_back_restores_it() {
    for pinned in [false, true] {
        let (mut canvas, mut previews, ids, viewport, aspects) = stretched_fixture();
        if pinned {
            canvas.toggle_viewport_pin(ids[0], viewport, &mut previews);
        }
        canvas.set_group_arrangement(ids[0], Equal, None, viewport, &mut previews);
        let frame = box_of(&canvas, &previews, ids[0], viewport);
        let equal = layout_of(&previews, &ids, viewport);
        for _ in 0..5 {
            for arrangement in ARRANGEMENTS {
                canvas.set_group_arrangement(ids[0], arrangement, None, viewport, &mut previews);
                let shown = screen_of(&canvas, &previews, &ids, viewport);
                assert_tidy(&shown, &aspects, frame, &format!("{arrangement:?}"));
                assert_rects(
                    &[box_of(&canvas, &previews, ids[0], viewport)],
                    &[frame],
                    0.05,
                    "box",
                );
            }
        }
        assert_rects(&layout_of(&previews, &ids, viewport), &equal, 0.05, "back");
        settle(&mut canvas, &mut previews, viewport, 1.0);
        assert_rects(
            &layout_of(&previews, &ids, viewport),
            &equal,
            0.05,
            "settled",
        );
    }
}

#[test]
fn the_chosen_tile_takes_a_large_slot() {
    let (mut canvas, mut previews, ids, viewport) = fixture();
    canvas.set_group_arrangement(ids[0], Main(2), Some(ids[0]), viewport, &mut previews);
    canvas.set_group_arrangement(ids[0], Main(2), Some(ids[3]), viewport, &mut previews);
    assert_eq!(
        &canvas.group_members(ids[0], &previews)[..2],
        &[ids[0], ids[3]]
    );
    canvas.set_group_arrangement(ids[0], Main(1), Some(ids[3]), viewport, &mut previews);
    assert_eq!(canvas.group_members(ids[0], &previews)[0], ids[3]);
    assert_eq!(canvas.current_tile(&previews), Some(ids[3]));
    let rects = layout_of(&previews, &ids, viewport);
    let largest = rects.iter().map(Rect::area).fold(0.0, f32::max);
    assert_eq!(rects[3].area(), largest);
}

#[test]
fn arranged_tiles_show_their_sources_unstretched() {
    for pinned in [false, true] {
        let (mut canvas, mut previews, ids, viewport, aspects) = stretched_fixture();
        if pinned {
            canvas.toggle_viewport_pin(ids[0], viewport, &mut previews);
        }
        for arrangement in ARRANGEMENTS {
            for selected in [ids[0], ids[1]] {
                canvas.set_group_arrangement(
                    ids[0],
                    arrangement,
                    Some(selected),
                    viewport,
                    &mut previews,
                );
                let rects = screen_of(&canvas, &previews, &ids, viewport);
                let frame = box_of(&canvas, &previews, ids[0], viewport);
                assert_tidy(&rects, &aspects, frame, &format!("{arrangement:?}"));
                if arrangement == Main(1) {
                    let largest = rects.iter().map(Rect::area).fold(0.0, f32::max);
                    let index = ids.iter().position(|id| *id == selected).unwrap();
                    assert_eq!(rects[index].area(), largest);
                }
            }
        }
    }
}

#[test]
fn swapping_tiles_back_and_forth_never_shrinks_them() {
    // Swapping a portrait and an ultrawide tile used to shrink both about
    // fivefold per swap.
    for arrangement in [Main(1), Equal] {
        let (mut canvas, mut previews, ids, viewport, aspects) = stretched_fixture();
        canvas.set_group_arrangement(ids[0], arrangement, None, viewport, &mut previews);
        let start = layout_of(&previews, &ids, viewport);
        let frame = box_of(&canvas, &previews, ids[0], viewport);
        for round in 1..=8 {
            canvas.swap_member_order(ids[0], ids[1], viewport, &mut previews);
            let rects = screen_of(&canvas, &previews, &ids, viewport);
            assert_tidy(&rects, &aspects, frame, &format!("swap {round}"));
            if round % 2 == 0 {
                assert_rects(
                    &layout_of(&previews, &ids, viewport),
                    &start,
                    0.05,
                    &format!("swap {round}"),
                );
            }
        }
    }
}

#[test]
fn members_leaving_and_joining_reflow_inside_the_same_box() {
    for pinned in [false, true] {
        let (mut canvas, mut previews, ids, viewport) = fixture();
        if pinned {
            canvas.toggle_viewport_pin(ids[0], viewport, &mut previews);
        }
        canvas.set_group_arrangement(ids[0], Equal, None, viewport, &mut previews);
        settle(&mut canvas, &mut previews, viewport, 1.0);
        let frame = box_of(&canvas, &previews, ids[0], viewport);
        let start = layout_of(&previews, &ids, viewport);
        let start_on_screen = screen_of(&canvas, &previews, &ids, viewport);
        let group = previews.get(ids[0]).unwrap().tile_group.clone().unwrap();

        // Leaving: the others re-flow into the same box and never shrink.
        previews.get_mut(ids[3]).unwrap().tile_group = None;
        settle(&mut canvas, &mut previews, viewport, 1.0);
        let three = screen_of(&canvas, &previews, &ids[..3], viewport);
        assert_tidy(&three, &[TILE_ASPECT; 3], frame, "three");
        assert!(
            three[0].area() >= canvas.to_screen_space(start[0], pinned, viewport).area() * 0.999
        );
        assert_rects(
            &[box_of(&canvas, &previews, ids[0], viewport)],
            &[frame],
            0.05,
            "box",
        );

        // Joining again: the tiles shrink to fit the same box, and the group
        // returns exactly to where it was.
        canvas.assign_group(&ids, group.clone(), viewport, &mut previews);
        settle(&mut canvas, &mut previews, viewport, 1.0);
        assert_rects(
            &layout_of(&previews, &ids, viewport),
            &start,
            0.05,
            "rejoined",
        );

        // A fifth tile fits the same box too. (It is not pinned, so the group
        // stops being pinned, staying where it is on screen.)
        let extra = previews.add("Extra".into(), Pos2::new(900.0, 600.0), Vec2::splat(120.0));
        let mut five = ids.clone();
        five.push(extra);
        canvas.assign_group(&five, group, viewport, &mut previews);
        settle(&mut canvas, &mut previews, viewport, 1.0);
        let mut aspects = vec![TILE_ASPECT; 4];
        aspects.push(1.0);
        let rects = screen_of(&canvas, &previews, &five, viewport);
        assert_tidy(&rects, &aspects, frame, "five");

        // Deleting one makes room the same way.
        previews.start_removal(extra);
        settle(&mut canvas, &mut previews, viewport, 1.0);
        assert_rects(
            &screen_of(&canvas, &previews, &ids, viewport),
            &start_on_screen,
            0.05,
            "deleted",
        );
    }
}

#[test]
fn a_source_that_changes_shape_reflows_once_the_shape_holds() {
    let (mut canvas, mut previews, ids, viewport, mut aspects) = stretched_fixture();
    canvas.update_layout_animation(10.0);
    canvas.set_group_arrangement(ids[0], Equal, None, viewport, &mut previews);
    settle(&mut canvas, &mut previews, viewport, 10.0);
    let frame = box_of(&canvas, &previews, ids[0], viewport);
    let before = layout_of(&previews, &ids, viewport);
    previews.get_mut(ids[1]).unwrap().source_frame_size = Some((1080, 1080));
    let wake = canvas.update_group_layouts(viewport, &mut previews, 10.1);
    assert!(wake.is_some_and(|at| (at - 10.7).abs() < 0.001), "{wake:?}");
    settle(&mut canvas, &mut previews, viewport, 10.5);
    assert_rects(
        &layout_of(&previews, &ids, viewport),
        &before,
        0.001,
        "early",
    );
    canvas.update_layout_animation(10.8);
    settle(&mut canvas, &mut previews, viewport, 10.8);
    aspects[1] = 1.0;
    let rects = screen_of(&canvas, &previews, &ids, viewport);
    assert_tidy(&rects, &aspects, frame, "settled");
    assert!(canvas.layout_is_animating());
}

#[test]
fn moving_scaling_pinning_and_panning_never_reflow_a_group() {
    let (mut canvas, mut previews, ids, viewport) = fixture();
    canvas.set_group_arrangement(ids[0], Main(1), Some(ids[0]), viewport, &mut previews);
    settle(&mut canvas, &mut previews, viewport, 1.0);
    canvas.selection = ids.clone();
    let check = |canvas: &mut CanvasState, previews: &mut PreviewManager, label: &str| {
        let before = layout_of(previews, &ids, viewport);
        let frame = box_of(canvas, previews, ids[0], viewport);
        settle(canvas, previews, viewport, 1.0);
        assert_rects(&layout_of(previews, &ids, viewport), &before, 0.001, label);
        assert_rects(
            &[box_of(canvas, previews, ids[0], viewport)],
            &[frame],
            0.01,
            label,
        );
    };
    canvas.move_tile_by(ids[2], Vec2::new(140.0, -60.0), &mut previews);
    check(&mut canvas, &mut previews, "moved");
    canvas.resize_selection(1440.0, viewport, &mut previews);
    check(&mut canvas, &mut previews, "scaled");
    canvas.pan += Vec2::new(-300.0, 80.0);
    canvas.zoom = 1.6;
    check(&mut canvas, &mut previews, "panned");
    canvas.zoom = 0.5;
    let shown = screen_of(&canvas, &previews, &ids, viewport);
    canvas.toggle_viewport_pin(ids[0], viewport, &mut previews);
    assert_rects(
        &screen_of(&canvas, &previews, &ids, viewport),
        &shown,
        0.01,
        "pin",
    );
    check(&mut canvas, &mut previews, "pinned");
    canvas.move_tile_by(ids[1], Vec2::new(-20.0, 15.0), &mut previews);
    check(&mut canvas, &mut previews, "pinned move");
    let smaller = Rect::from_min_size(viewport.min, Vec2::new(900.0, 700.0));
    let before = layout_of(&previews, &ids, smaller);
    settle(&mut canvas, &mut previews, smaller, 1.0);
    assert_rects(
        &layout_of(&previews, &ids, smaller),
        &before,
        0.001,
        "resized window",
    );
    assert_eq!(arrangement_of(&previews, ids[0]), Main(1));
}

#[test]
fn a_tile_changed_on_its_own_goes_back_into_the_box_the_others_agree_on() {
    for count in [2, 4] {
        let (mut canvas, mut previews, mut ids, viewport) = fixture();
        for id in ids.drain(count..) {
            previews.get_mut(id).unwrap().tile_group = None;
        }
        canvas.set_group_arrangement(ids[0], Equal, None, viewport, &mut previews);
        settle(&mut canvas, &mut previews, viewport, 1.0);
        let frame = box_of(&canvas, &previews, ids[0], viewport);
        // Like a crop that keeps the left half of the first tile.
        previews.get_mut(ids[0]).unwrap().size.x *= 0.5;
        settle(&mut canvas, &mut previews, viewport, 1.0);
        assert_rects(
            &[box_of(&canvas, &previews, ids[0], viewport)],
            &[frame],
            0.05,
            "box",
        );
        let mut aspects = vec![TILE_ASPECT; count];
        aspects[0] *= 0.5;
        let rects = screen_of(&canvas, &previews, &ids, viewport);
        assert_tidy(&rects, &aspects, frame, &format!("{count} tiles"));
    }
}

#[test]
fn make_main_tile_and_cycling_change_the_large_tile() {
    let (mut canvas, mut previews, ids, viewport) = fixture();
    canvas.set_group_arrangement(ids[0], Equal, None, viewport, &mut previews);
    canvas.make_main_tile(ids[2], viewport, &mut previews);
    assert_eq!(arrangement_of(&previews, ids[0]), Main(1));
    assert_eq!(canvas.group_members(ids[0], &previews)[0], ids[2]);
    canvas.cycle_main_tile(true, Some(ids[2]), viewport, &mut previews);
    let order = canvas.group_members(ids[0], &previews);
    assert_eq!(&order[3..], &[ids[2]]);
    assert_eq!(canvas.current_tile(&previews), Some(order[0]));
    canvas.cycle_main_tile(false, Some(order[0]), viewport, &mut previews);
    assert_eq!(canvas.group_members(ids[0], &previews)[0], ids[2]);
    let rects = layout_of(&previews, &ids, viewport);
    let largest = rects.iter().map(Rect::area).fold(0.0, f32::max);
    assert_eq!(rects[2].area(), largest);

    // Without large tiles, cycling only moves the current tile.
    canvas.set_group_arrangement(ids[0], Equal, None, viewport, &mut previews);
    let before = layout_of(&previews, &ids, viewport);
    let current = canvas.current_tile(&previews).unwrap();
    canvas.cycle_main_tile(true, Some(current), viewport, &mut previews);
    assert_ne!(canvas.current_tile(&previews), Some(current));
    assert_rects(
        &layout_of(&previews, &ids, viewport),
        &before,
        0.001,
        "equal",
    );
}

#[test]
fn layout_animation_retargets_without_jumping_or_saving_intermediate_geometry() {
    for pinned in [false, true] {
        let (mut canvas, mut previews, ids, viewport) = fixture();
        if pinned {
            canvas.toggle_viewport_pin(ids[0], viewport, &mut previews);
        }
        let before = canvas.preview_screen_rect(previews.get(ids[0]).unwrap(), viewport);
        canvas.update_layout_animation(1.0);
        canvas.set_group_arrangement(ids[0], Main(2), Some(ids[1]), viewport, &mut previews);
        assert!(canvas.layout_is_animating());
        let shown = canvas.preview_screen_rect(previews.get(ids[0]).unwrap(), viewport);
        assert!(shown.min.distance(before.min) < 0.001);
        canvas.update_layout_animation(1.09);
        let midpoint = canvas.preview_screen_rect(previews.get(ids[0]).unwrap(), viewport);
        canvas.set_group_arrangement(ids[0], Main(3), None, viewport, &mut previews);
        let retarget = canvas.preview_screen_rect(previews.get(ids[0]).unwrap(), viewport);
        assert!(retarget.min.distance(midpoint.min) < 0.001);
        assert!((retarget.size() - midpoint.size()).length() < 0.001);
        let p = previews.get(ids[0]).unwrap();
        assert_eq!(PreviewLayout::from(p).size, (p.size.x, p.size.y));
        canvas.update_layout_animation(1.5);
        assert!(!canvas.layout_is_animating());
        let after = canvas.preview_screen_rect(previews.get(ids[0]).unwrap(), viewport);
        let target = canvas.target_screen_rect(previews.get(ids[0]).unwrap(), viewport);
        assert_eq!(after, target);
    }
}

#[test]
fn content_aspect_follows_the_source_frame_and_crop() {
    let (_, mut previews, ids, _, aspects) = stretched_fixture();
    for (id, aspect) in ids.iter().zip(aspects) {
        let found = content_aspect(previews.get(*id).unwrap()).unwrap();
        assert!((found - aspect).abs() < 0.001, "{found} vs {aspect}");
    }
    let p = previews.get_mut(ids[0]).unwrap();
    p.browser_url = Some("https://example.com".into());
    assert_eq!(content_aspect(p), None);
    let p = previews.get_mut(ids[1]).unwrap();
    p.source_frame_size = None;
    assert_eq!(content_aspect(p), None);
}

#[test]
fn group_selection_member_edit_and_ungroup_preserve_geometry() {
    let (mut canvas, mut previews, ids, _) = fixture();
    let before: Vec<_> = ids
        .iter()
        .map(|id| previews.get(*id).unwrap().rect())
        .collect();
    canvas.select_group_tile(ids[1], &previews);
    assert_eq!(canvas.selection, ids);
    canvas.toggle_group_selection(ids[2], &previews);
    assert!(canvas.selection.is_empty());
    canvas.toggle_group_selection(ids[2], &previews);
    assert_eq!(canvas.selection, ids);
    canvas.groups.editing = Some(ids[1]);
    canvas.select_group_tile(ids[1], &previews);
    assert_eq!(canvas.selection, vec![ids[1]]);
    canvas.select_group_tile(ids[0], &previews);
    assert_eq!(canvas.selection, ids);
    canvas.ungroup_selection(&mut previews);
    for (id, before) in ids.iter().zip(before) {
        let p = previews.get(*id).unwrap();
        assert_eq!(p.rect(), before);
        assert!(p.tile_group.is_none());
    }
}

#[test]
fn group_pin_survives_pan_zoom_and_viewport_resize_as_one_unit() {
    let (mut canvas, mut previews, ids, viewport) = fixture();
    let before: Vec<_> = ids
        .iter()
        .map(|id| canvas.preview_screen_rect(previews.get(*id).unwrap(), viewport))
        .collect();
    canvas.toggle_viewport_pin(ids[0], viewport, &mut previews);
    canvas.pan = Vec2::new(-900.0, 400.0);
    canvas.zoom = 3.0;
    let resized = Rect::from_min_size(viewport.min, Vec2::new(600.0, 420.0));
    for (id, before) in ids.iter().zip(&before) {
        assert_eq!(
            canvas.preview_screen_rect(previews.get(*id).unwrap(), resized),
            *before
        );
    }
    canvas.toggle_viewport_pin(ids[3], resized, &mut previews);
    for (id, before) in ids.iter().zip(before) {
        let p = previews.get(*id).unwrap();
        assert!(p.viewport_pin.is_none());
        let after = canvas.preview_screen_rect(p, resized);
        assert!(after.min.distance(before.min) < 0.001);
        assert!((after.size() - before.size()).length() < 0.001);
    }
}

#[test]
fn grouping_mixed_pins_keeps_current_screen_positions() {
    let (mut canvas, mut previews, ids, viewport) = fixture();
    canvas.ungroup_selection(&mut previews);
    canvas.toggle_viewport_pin(ids[0], viewport, &mut previews);
    let before: Vec<_> = ids
        .iter()
        .map(|id| canvas.preview_screen_rect(previews.get(*id).unwrap(), viewport))
        .collect();
    canvas.create_group(viewport, &mut previews);
    for (id, before) in ids.iter().zip(before) {
        let p = previews.get(*id).unwrap();
        assert!(p.viewport_pin.is_none());
        let after = canvas.preview_screen_rect(p, viewport);
        assert!(after.min.distance(before.min) < 0.001);
        assert!((after.size() - before.size()).length() < 0.001);
    }
}

#[test]
fn focus_uses_the_clicked_group_member() {
    let (mut canvas, previews, ids, viewport) = fixture();
    canvas.select_group_tile(ids[2], &previews);
    assert_eq!(canvas.selection, ids);
    assert_eq!(canvas.current_tile(&previews), Some(ids[2]));
    let camera = (canvas.pan, canvas.zoom);
    canvas.toggle_current_tile_focus(&previews);
    assert_eq!(canvas.focus.as_ref().unwrap().id, ids[2]);
    let focused = canvas.preview_screen_rect(previews.get(ids[2]).unwrap(), viewport);
    assert!(focused.center().distance(viewport.center()) < 0.001);
    canvas.toggle_current_tile_focus(&previews);
    assert_eq!(canvas.current_tile(&previews), Some(ids[2]));
    assert_eq!((canvas.pan, canvas.zoom), camera);
}

#[test]
fn small_group_member_focus_fills_view_beyond_navigation_zoom_limit() {
    let (mut canvas, mut previews, ids, viewport) = fixture();
    previews.get_mut(ids[3]).unwrap().size = Vec2::new(40.0, 25.0);
    canvas.select_group_tile(ids[3], &previews);
    let camera = (canvas.pan, canvas.zoom);
    canvas.toggle_current_tile_focus(&previews);
    assert!(canvas.zoom > canvas.zoom_max);
    let focused = canvas.preview_screen_rect(previews.get(ids[3]).unwrap(), viewport);
    assert!((focused.width() - viewport.width()).abs() < 0.001);
    canvas.toggle_current_tile_focus(&previews);
    assert_eq!((canvas.pan, canvas.zoom), camera);
}

#[test]
fn regrouping_preserves_name_and_adding_members_preserves_one_identity() {
    let (mut canvas, mut previews, ids, viewport) = fixture();
    let mut group = previews.get(ids[0]).unwrap().tile_group.clone().unwrap();
    group.name = "Streams".into();
    canvas.assign_group(&ids, group.clone(), viewport, &mut previews);
    canvas.create_group(viewport, &mut previews);
    assert_eq!(
        previews
            .get(ids[0])
            .unwrap()
            .tile_group
            .as_ref()
            .unwrap()
            .name,
        "Streams"
    );
    let added = previews.add("Extra".into(), Pos2::new(20.0, 500.0), Vec2::splat(100.0));
    let mut members = ids.clone();
    members.push(added);
    canvas.assign_group(&members, group.clone(), viewport, &mut previews);
    assert_eq!(canvas.group_members(added, &previews), members);
    assert_eq!(
        previews.get(added).unwrap().tile_group.as_ref().unwrap().id,
        group.id
    );
    canvas.groups.editing = Some(added);
    canvas.selection = vec![added];
    canvas.ungroup_selection(&mut previews);
    assert!(members
        .iter()
        .all(|id| previews.get(*id).unwrap().tile_group.is_none()));
}

#[test]
fn new_groups_get_distinct_names() {
    let (mut canvas, mut previews, ids, viewport) = fixture();
    let name = |previews: &PreviewManager, id| {
        previews
            .get(id)
            .unwrap()
            .tile_group
            .as_ref()
            .unwrap()
            .name
            .clone()
    };
    assert_eq!(name(&previews, ids[0]), "Group 1");
    let more: Vec<_> = (0..2)
        .map(|i| {
            previews.add(
                format!("More {i}"),
                Pos2::new(900.0 + i as f32 * 200.0, 0.0),
                Vec2::splat(150.0),
            )
        })
        .collect();
    canvas.selection = more.clone();
    canvas.create_group(viewport, &mut previews);
    assert_eq!(name(&previews, more[0]), "Group 2");
}

#[test]
fn arrangements_leave_crop_and_other_tiles_unchanged() {
    let (mut canvas, mut previews, ids, viewport) = fixture();
    previews.get_mut(ids[0]).unwrap().crop_uv = Some((0.25, 0.0, 0.75, 1.0));
    let outsider = previews.add("Outside".into(), Pos2::new(-100.0, 20.0), Vec2::splat(30.0));
    let before = previews.get(outsider).unwrap().rect();
    for (arrangement, _, _) in PRESETS {
        canvas.set_group_arrangement(ids[0], arrangement, Some(ids[0]), viewport, &mut previews);
        settle(&mut canvas, &mut previews, viewport, 1.0);
        assert_eq!(
            previews.get(ids[0]).unwrap().crop_uv,
            Some((0.25, 0.0, 0.75, 1.0))
        );
        assert_eq!(previews.get(outsider).unwrap().rect(), before);
    }
}

#[test]
fn older_workspaces_load_groups_as_free_and_new_fields_round_trip() {
    let (mut canvas, mut previews, ids, viewport) = fixture();
    let saved = PreviewLayout::from(previews.get(ids[0]).unwrap());
    let mut value = serde_json::to_value(&saved).unwrap();
    for field in ["arrangement", "gap", "frame"] {
        value["tile_group"].as_object_mut().unwrap().remove(field);
    }
    let group = serde_json::from_value::<PreviewLayout>(value.clone())
        .unwrap()
        .tile_group
        .unwrap();
    assert_eq!(
        (group.arrangement, group.gap, group.frame),
        (Free, GroupGap::Small, None)
    );
    value.as_object_mut().unwrap().remove("tile_group");
    assert!(serde_json::from_value::<PreviewLayout>(value)
        .unwrap()
        .tile_group
        .is_none());

    canvas.set_group_arrangement(ids[0], Main(2), None, viewport, &mut previews);
    canvas.set_group_gap(ids[0], GroupGap::Large, viewport, &mut previews);
    let saved = PreviewLayout::from(previews.get(ids[0]).unwrap());
    let restored: PreviewLayout =
        serde_json::from_value(serde_json::to_value(&saved).unwrap()).unwrap();
    assert_eq!(restored.tile_group, saved.tile_group);
    let group = restored.tile_group.unwrap();
    assert_eq!((group.arrangement, group.gap), (Main(2), GroupGap::Large));
    assert!(group.frame.is_some());
}

#[test]
fn restacking_moves_the_whole_group_and_keeps_its_inner_order() {
    let (mut canvas, mut previews, ids, _) = fixture();
    let outsider = previews.add("Outside".into(), Pos2::new(-100.0, 20.0), Vec2::splat(30.0));
    let z = |previews: &PreviewManager, id| previews.get(id).unwrap().z_order;
    previews.bring_to_front(ids[1]);
    canvas.restack_group(ids[2], false, &mut previews);
    assert!(ids
        .iter()
        .all(|id| z(&previews, *id) < z(&previews, outsider)));
    assert!(z(&previews, ids[1]) > z(&previews, ids[3]));
    canvas.restack_group(ids[2], true, &mut previews);
    assert!(ids
        .iter()
        .all(|id| z(&previews, *id) > z(&previews, outsider)));
    assert!(z(&previews, ids[1]) > z(&previews, ids[3]));
}

#[test]
fn group_resize_floor_lets_an_already_small_group_grow() {
    let tiny = [(
        PreviewId(1),
        Rect::from_min_size(Pos2::ZERO, Vec2::splat(40.0)),
    )];
    assert_eq!(group_resize_min_scale(&tiny, 1.0), 1.0);
    let large = [(
        PreviewId(1),
        Rect::from_min_size(Pos2::ZERO, Vec2::splat(400.0)),
    )];
    assert!((group_resize_min_scale(&large, 1.0) - 0.25).abs() < 0.001);
    assert!((group_resize_min_scale(&large, 2.0) - 0.5).abs() < 0.001);
}

#[test]
fn fit_group_to_view_fills_the_window() {
    for arranged in [false, true] {
        for pinned in [false, true] {
            let (mut canvas, mut previews, ids, viewport) = fixture();
            if pinned {
                canvas.toggle_viewport_pin(ids[0], viewport, &mut previews);
            }
            if arranged {
                canvas.set_group_arrangement(ids[0], Equal, None, viewport, &mut previews);
            }
            let before = screen_of(&canvas, &previews, &ids, viewport);
            let old = bounds_of(before.iter().copied()).unwrap();
            // Shrink the group to a few pixels.
            for (id, rect) in ids.iter().zip(&before) {
                canvas.set_group_screen_rect(
                    *id,
                    Rect::from_min_size(old.min + (rect.min - old.min) * 0.02, rect.size() * 0.02),
                    viewport,
                    &mut previews,
                );
            }
            canvas.fit_group_to_view(ids[0], viewport, &mut previews);
            let after = screen_of(&canvas, &previews, &ids, viewport);
            let fitted = bounds_of(after.iter().copied()).unwrap();
            let context = format!("arranged={arranged} pinned={pinned}");
            assert!(viewport.contains_rect(fitted), "{context}");
            assert!(
                fitted.width() > viewport.width() * 0.9
                    || fitted.height() > viewport.height() * 0.9,
                "{context}"
            );
            assert!(
                (fitted.center() - viewport.center()).length() < 0.5,
                "{context}"
            );
            if !arranged {
                let scale = fitted.width() / old.width();
                for (before, after) in before.iter().zip(&after) {
                    assert!((after.size() - before.size() * scale).length() < 0.5);
                    assert!(
                        ((after.min - fitted.min) - (before.min - old.min) * scale).length() < 0.5
                    );
                }
            }
            for id in &ids {
                assert_eq!(previews.get(*id).unwrap().viewport_pin.is_some(), pinned);
            }
        }
    }
}

#[test]
fn keep_in_view_slides_back_only_as_far_as_the_window_or_old_outline() {
    let viewport = Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 600.0));
    let previous = Rect::from_min_size(Pos2::new(700.0, 100.0), Vec2::new(200.0, 400.0));
    let moved = keep_in_view(
        vec![Rect::from_min_size(previous.min, Vec2::new(500.0, 300.0))],
        previous,
        viewport,
    );
    assert_eq!(moved[0].min, Pos2::new(500.0, 100.0));
    // A group already parked past the edge is not pulled into the window.
    let parked = Rect::from_min_size(Pos2::new(900.0, 100.0), Vec2::new(400.0, 200.0));
    let moved = keep_in_view(
        vec![Rect::from_min_size(parked.min, Vec2::new(300.0, 300.0))],
        parked,
        viewport,
    );
    assert_eq!(moved[0].min, parked.min);
    // Wider than the window: keep the left edge inside it.
    let moved = keep_in_view(
        vec![Rect::from_min_size(previous.min, Vec2::new(1500.0, 100.0))],
        previous,
        viewport,
    );
    assert_eq!(moved[0].min, Pos2::new(0.0, 100.0));
}

#[test]
fn arranging_keeps_a_visible_group_inside_the_window() {
    for pinned in [false, true] {
        let (mut canvas, mut previews, ids, viewport) = fixture();
        // A tall column against the window's right edge.
        for (i, id) in ids.iter().enumerate() {
            canvas.set_group_screen_rect(
                *id,
                Rect::from_min_size(
                    Pos2::new(
                        viewport.right() - 170.0,
                        viewport.top() + 10.0 + i as f32 * 95.0,
                    ),
                    Vec2::new(160.0, 90.0),
                ),
                viewport,
                &mut previews,
            );
        }
        if pinned {
            canvas.toggle_viewport_pin(ids[0], viewport, &mut previews);
        }
        for (arrangement, _, _) in PRESETS {
            canvas.set_group_arrangement(
                ids[0],
                arrangement,
                Some(ids[0]),
                viewport,
                &mut previews,
            );
            for rect in screen_of(&canvas, &previews, &ids, viewport) {
                assert!(
                    viewport.expand(0.5).contains_rect(rect),
                    "{arrangement:?}: {rect:?}"
                );
            }
        }
    }
}

#[test]
fn size_presets_use_canvas_units_or_pinned_screen_units() {
    for pinned in [false, true] {
        let (mut canvas, mut previews, ids, viewport) = fixture();
        if pinned {
            canvas.toggle_viewport_pin(ids[0], viewport, &mut previews);
        }
        for (width, _, _) in SIZES {
            canvas.resize_selection(width, viewport, &mut previews);
            let bounds =
                bounds_of(screen_of(&canvas, &previews, &ids, viewport).into_iter()).unwrap();
            assert!((bounds.width() - width * if pinned { 1.0 } else { canvas.zoom }).abs() < 0.01);
        }
    }
}

fn frame_with(
    context: &egui::Context,
    canvas: &mut CanvasState,
    previews: &mut PreviewManager,
    captures: &mut CaptureCoordinator,
    events: Vec<egui::Event>,
    modifiers: egui::Modifiers,
) {
    let _ = context.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1400.0, 900.0))),
            events,
            modifiers,
            ..Default::default()
        },
        |ui| {
            let ctx = &ui.ctx().clone();
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ui, |ui| {
                    canvas.ui(ui, previews, captures, ctx, true);
                });
        },
    );
}

fn frame(
    context: &egui::Context,
    canvas: &mut CanvasState,
    previews: &mut PreviewManager,
    captures: &mut CaptureCoordinator,
    events: Vec<egui::Event>,
) {
    frame_with(
        context,
        canvas,
        previews,
        captures,
        events,
        egui::Modifiers::NONE,
    );
}

/// Press at `start`, move through `path` and release at its end.
fn pointer_drag(
    context: &egui::Context,
    canvas: &mut CanvasState,
    previews: &mut PreviewManager,
    captures: &mut CaptureCoordinator,
    start: Pos2,
    path: &[Pos2],
    modifiers: egui::Modifiers,
) {
    let end = *path.last().unwrap();
    let press = |pos, pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers,
    };
    let mut steps = vec![
        vec![egui::Event::PointerMoved(start)],
        vec![press(start, true)],
    ];
    steps.extend(path.iter().map(|at| vec![egui::Event::PointerMoved(*at)]));
    steps.push(vec![press(end, false)]);
    for events in steps {
        frame_with(context, canvas, previews, captures, events, modifiers);
    }
}

struct UiFixture {
    canvas: CanvasState,
    previews: PreviewManager,
    ids: Vec<PreviewId>,
    viewport: Rect,
    context: egui::Context,
    captures: CaptureCoordinator,
}

impl UiFixture {
    /// A group with `arrangement`, drawn once so pointer input reaches it.
    fn new(arrangement: GroupArrangement, pinned: bool) -> Self {
        let (mut canvas, mut previews, ids, _) = fixture();
        // Fully inside the window, with room above for the group bar.
        canvas.pan = Vec2::new(100.0, 120.0);
        let context = egui::Context::default();
        let mut captures = CaptureCoordinator::new();
        for id in &ids {
            previews.get_mut(*id).unwrap().created_at = Instant::now() - Duration::from_secs(1);
        }
        frame(&context, &mut canvas, &mut previews, &mut captures, vec![]);
        let viewport = canvas.last_screen_rect.unwrap();
        if pinned {
            canvas.toggle_viewport_pin(ids[0], viewport, &mut previews);
        }
        if arrangement != Free {
            canvas.set_group_arrangement(
                ids[0],
                arrangement,
                Some(ids[0]),
                viewport,
                &mut previews,
            );
        }
        canvas.update_layout_animation(10.0);
        let mut fixture = Self {
            canvas,
            previews,
            ids,
            viewport,
            context,
            captures,
        };
        fixture.frame();
        fixture
    }

    fn frame(&mut self) {
        frame(
            &self.context,
            &mut self.canvas,
            &mut self.previews,
            &mut self.captures,
            vec![],
        );
    }

    fn drag(&mut self, start: Pos2, path: &[Pos2], modifiers: egui::Modifiers) {
        pointer_drag(
            &self.context,
            &mut self.canvas,
            &mut self.previews,
            &mut self.captures,
            start,
            path,
            modifiers,
        );
    }

    fn screen(&self, ids: &[PreviewId]) -> Vec<Rect> {
        screen_of(&self.canvas, &self.previews, ids, self.viewport)
    }

    fn select_group(&mut self) {
        self.canvas.select_group_tile(self.ids[0], &self.previews);
        self.frame();
    }
}

#[test]
fn pointer_drag_moves_all_group_members_with_and_without_pins() {
    for arrangement in [Free, Main(1)] {
        for pinned in [false, true] {
            let mut ui = UiFixture::new(arrangement, pinned);
            ui.canvas.selection.clear();
            let ids = ui.ids.clone();
            let before = ui.screen(&ids);
            let start = before[0].center();
            ui.drag(
                start,
                &[start + Vec2::new(30.0, 20.0), start + Vec2::new(60.0, 40.0)],
                egui::Modifiers::NONE,
            );
            ui.frame();
            for (before, after) in before.iter().zip(ui.screen(&ids)) {
                assert!(
                    ((after.min - before.min) - Vec2::new(60.0, 40.0)).length() < 0.01,
                    "{arrangement:?} pinned={pinned}: before={before:?} after={after:?}"
                );
            }
            assert_eq!(arrangement_of(&ui.previews, ids[0]), arrangement);
        }
    }
}

#[test]
fn letting_go_of_alt_before_the_button_ends_a_group_tile_crop() {
    let mut ui = UiFixture::new(Free, false);
    for id in ui.ids.clone() {
        ui.previews
            .get_mut(id)
            .unwrap()
            .update_frame(300, 180, vec![0; 300 * 180 * 4]);
    }
    ui.select_group();
    let tile = ui.screen(&[ui.ids[0]])[0];
    // The left edge handle: no other member sits to the left of tile 0.
    let start = tile.left_center();
    let alt = egui::Modifiers::ALT;
    let press = |pos, pressed, modifiers| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers,
    };
    let step = |ui: &mut UiFixture, events, modifiers| {
        frame_with(
            &ui.context,
            &mut ui.canvas,
            &mut ui.previews,
            &mut ui.captures,
            events,
            modifiers,
        );
    };
    step(&mut ui, vec![egui::Event::PointerMoved(start)], alt);
    step(&mut ui, vec![press(start, true, alt)], alt);
    for x in [10.0, 20.0, 30.0] {
        step(
            &mut ui,
            vec![egui::Event::PointerMoved(start + Vec2::new(x, 0.0))],
            alt,
        );
    }
    assert!(
        matches!(ui.canvas.drag_state, Some(DragState::Cropping { .. })),
        "the Alt drag should crop the tile: {:?}",
        ui.canvas.drag_state.is_some()
    );
    // Alt comes up first, which swaps the tile handles for the group outline.
    let end = start + Vec2::new(40.0, 0.0);
    step(
        &mut ui,
        vec![egui::Event::PointerMoved(end)],
        egui::Modifiers::NONE,
    );
    step(
        &mut ui,
        vec![press(end, false, egui::Modifiers::NONE)],
        egui::Modifiers::NONE,
    );
    ui.frame();

    assert!(
        ui.canvas.drag_state.is_none(),
        "a finished crop must not keep blocking pan, zoom and box selection"
    );
}

#[test]
fn a_handle_drag_on_a_removed_tile_ends() {
    let (mut canvas, mut previews, ids, _) = fixture();
    let rect = previews.get(ids[0]).unwrap().rect();
    canvas.drag_state = Some(DragState::Resizing {
        id: ids[0],
        handle: ResizeHandle::Right,
        start_rect: rect,
        start_mouse: rect.right_center(),
        aspect_ratio: None,
        screen_space: false,
    });
    canvas.end_orphaned_handle_drag(true, &previews);
    assert!(canvas.drag_state.is_some(), "a live drag keeps going");

    previews.clear();
    canvas.end_orphaned_handle_drag(true, &previews);
    assert!(canvas.drag_state.is_none());
}

#[test]
fn shift_drag_swaps_two_tiles_and_an_outside_drop_cancels() {
    for pinned in [false, true] {
        let mut ui = UiFixture::new(Main(1), pinned);
        for outside in [false, true] {
            let before = ui.canvas.group_members(ui.ids[0], &ui.previews);
            let slots = ui.screen(&before);
            let start = slots[0].center();
            let end = if outside {
                Pos2::new(-100.0, -100.0)
            } else {
                slots[3].center()
            };
            ui.drag(
                start,
                &[start + Vec2::new(20.0, 15.0), end],
                egui::Modifiers::SHIFT,
            );
            assert!(ui.canvas.groups.reorder.is_none());
            let mut expected = before.clone();
            if !outside {
                expected.swap(0, 3);
            }
            let context = format!("pinned={pinned} outside={outside}");
            assert_eq!(
                ui.canvas.group_members(ui.ids[0], &ui.previews),
                expected,
                "{context}"
            );
            // The group stayed put; the two tiles traded slots.
            assert_rects(&ui.screen(&expected), &slots, 0.05, &context);
            ui.canvas.update_layout_animation(20.0);
        }
    }
}

#[test]
fn dragging_one_member_on_its_own_frees_the_group() {
    let mut ui = UiFixture::new(Equal, false);
    let ids = ui.ids.clone();
    ui.canvas.groups.editing = Some(ids[1]);
    ui.canvas.selection = vec![ids[1]];
    let before = ui.screen(&ids);
    let start = before[1].center();
    ui.drag(
        start,
        &[start + Vec2::new(20.0, 10.0), start + Vec2::new(40.0, 30.0)],
        egui::Modifiers::NONE,
    );
    ui.frame();
    for (index, (before, after)) in before.iter().zip(ui.screen(&ids)).enumerate() {
        let expected = if index == 1 {
            Vec2::new(40.0, 30.0)
        } else {
            Vec2::ZERO
        };
        assert!(((after.min - before.min) - expected).length() < 0.01);
    }
    assert!(ids
        .iter()
        .all(|id| arrangement_of(&ui.previews, *id) == Free));
}

#[test]
fn dragging_the_box_edge_lays_an_arranged_group_out_again() {
    for pinned in [false, true] {
        let mut ui = UiFixture::new(Equal, pinned);
        let ids = ui.ids.clone();
        ui.select_group();
        assert_eq!(rows(&ui.screen(&ids)), 2);
        let outline = box_of(&ui.canvas, &ui.previews, ids[0], ui.viewport);
        let handle = outline.expand(3.0).right_center();
        ui.drag(
            handle,
            &[
                handle + Vec2::new(30.0, 0.0),
                handle + Vec2::new(800.0, 0.0),
            ],
            egui::Modifiers::NONE,
        );
        let rects = ui.screen(&ids);
        assert_eq!(rows(&rects), 1, "pinned={pinned}");
        let wider = box_of(&ui.canvas, &ui.previews, ids[0], ui.viewport);
        assert!((wider.width() - outline.width() - 800.0).abs() < 0.5);
        assert_tidy(&rects, &[TILE_ASPECT; 4], wider, "wider");

        // Dragging the edge far in stops before tiles get too small.
        let handle = wider.expand(3.0).right_center();
        ui.drag(
            handle,
            &[
                handle - Vec2::new(30.0, 0.0),
                Pos2::new(wider.left() + 5.0, handle.y),
            ],
            egui::Modifiers::NONE,
        );
        let unit = if pinned { 1.0 } else { ui.canvas.zoom };
        let smallest = smallest_side(&ui.screen(&ids)) / unit;
        assert!(
            smallest >= MIN_TILE_SIDE - 0.5,
            "pinned={pinned}: {smallest}"
        );
        ui.frame();
        assert_eq!(arrangement_of(&ui.previews, ids[0]), Equal);
    }
}

#[test]
fn pointer_group_resize_preserves_relative_geometry() {
    for pinned in [false, true] {
        let mut ui = UiFixture::new(Free, pinned);
        let ids = ui.ids.clone();
        ui.select_group();
        let before = ui.screen(&ids);
        let bounds = bounds_of(before.iter().copied()).unwrap();
        let end = bounds.max + bounds.size() * 0.3;
        ui.drag(bounds.max, &[end], egui::Modifiers::NONE);
        for (before, after) in before.iter().zip(ui.screen(&ids)) {
            assert!(
                (after.size() - before.size() * 1.3).length() < 0.01,
                "pinned={pinned}: {before:?} -> {after:?}"
            );
            assert!(((after.min - bounds.min) - (before.min - bounds.min) * 1.3).length() < 0.01);
        }
    }
}

#[test]
fn pointer_group_resize_from_the_top_left_keeps_the_opposite_corner() {
    let mut ui = UiFixture::new(Free, false);
    let ids = ui.ids.clone();
    ui.select_group();
    let bounds = bounds_of(ui.screen(&ids).into_iter()).unwrap();
    let end = bounds.min + bounds.size() * 0.2;
    ui.drag(bounds.min, &[end], egui::Modifiers::NONE);
    let after = bounds_of(ui.screen(&ids).into_iter()).unwrap();
    assert!(after.max.distance(bounds.max) < 0.01);
    assert!((after.width() / bounds.width() - 0.8).abs() < 0.01);
}

#[test]
fn pointer_group_resize_stops_at_the_minimum_tile_size() {
    for pinned in [false, true] {
        let mut ui = UiFixture::new(Free, pinned);
        let ids = ui.ids.clone();
        ui.select_group();
        let bounds = bounds_of(ui.screen(&ids).into_iter()).unwrap();
        let end = bounds.min + bounds.size() * 0.05;
        ui.drag(bounds.max, &[end], egui::Modifiers::NONE);
        let unit = if pinned { 1.0 } else { ui.canvas.zoom };
        let smallest = smallest_side(&ui.screen(&ids)) / unit;
        assert!(
            (smallest - MIN_TILE_SIDE).abs() < 0.5,
            "pinned={pinned}: {smallest}"
        );
    }
}

#[test]
fn a_hidden_group_is_one_entry_in_the_hidden_list() {
    let mut canvas = CanvasState::default();
    let viewport = Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 700.0));
    canvas.last_screen_rect = Some(viewport);
    let mut previews = PreviewManager::new();
    let ids: Vec<_> = (0..3)
        .map(|i| {
            previews.add(
                format!("Tile {i}"),
                Pos2::new(i as f32 * 220.0, 0.0),
                Vec2::new(200.0, 120.0),
            )
        })
        .collect();
    canvas.selection = ids[..2].to_vec();
    canvas.create_group(viewport, &mut previews);
    canvas.set_tiles_hidden(&ids, true, &mut previews);
    let entries = CanvasState::hidden_entries(&previews);
    assert_eq!(entries.len(), 2);
    let group = entries
        .iter()
        .find(|entry| entry.members.len() == 2)
        .expect("the group is one entry");
    assert!(group.label.ends_with(" · 2 tiles"), "{}", group.label);
    assert!(entries.iter().any(|entry| entry.label == "Tile 2"));
}
