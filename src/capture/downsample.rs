/// Same balanced oversample as parked browser tiles: 2× the on-tile pixels,
/// never larger than the source window, capped at 4K.
pub const CAPTURE_SCALE: f32 = 2.0;
pub const MAX_CAPTURE_WIDTH: u32 = 3840;
pub const MAX_CAPTURE_HEIGHT: u32 = 2160;

/// Canvas zoom below 100% shrinks capture backing. Zooming in past 100% does
/// not request more than the 2×-tile budget used at 1.0.
pub fn capture_lod_factor(canvas_zoom: f32) -> f32 {
    canvas_zoom.clamp(0.05, 1.0)
}

/// 2× the tile's physical pixels, inflated so a crop still has enough source
/// samples, then quantized to 32px so interactive resize does not rebuild the
/// GPU texture every pixel.
pub fn window_capture_target(
    tile_width: f32,
    tile_height: f32,
    pixels_per_point: f32,
    crop_uv: Option<(f32, f32, f32, f32)>,
) -> (u32, u32) {
    let mut width = tile_width.max(1.0) * pixels_per_point.max(0.01) * CAPTURE_SCALE;
    let mut height = tile_height.max(1.0) * pixels_per_point.max(0.01) * CAPTURE_SCALE;
    if let Some((min_u, min_v, max_u, max_v)) = crop_uv {
        width /= (max_u - min_u).clamp(0.05, 1.0);
        height /= (max_v - min_v).clamp(0.05, 1.0);
    }
    (quantize_dim(width), quantize_dim(height))
}

fn quantize_dim(value: f32) -> u32 {
    let value = value.max(1.0).round() as u32;
    value.saturating_add(31) / 32 * 32
}

/// Fit a source frame into a target box without upscaling or stretching.
/// A `0` target width or height means "keep the native source size".
pub fn fitted_capture_size(
    source_width: u32,
    source_height: u32,
    target_width: u32,
    target_height: u32,
) -> (u32, u32) {
    if source_width == 0 || source_height == 0 {
        return (source_width.max(1), source_height.max(1));
    }
    if target_width == 0 || target_height == 0 {
        return (source_width, source_height);
    }

    let box_width = target_width.min(MAX_CAPTURE_WIDTH).min(source_width);
    let box_height = target_height.min(MAX_CAPTURE_HEIGHT).min(source_height);
    let scale = (box_width as f64 / f64::from(source_width))
        .min(box_height as f64 / f64::from(source_height))
        .min(1.0);
    if scale >= 1.0 {
        (source_width, source_height)
    } else {
        (
            ((f64::from(source_width) * scale).round() as u32).max(1),
            ((f64::from(source_height) * scale).round() as u32).max(1),
        )
    }
}

/// Area-average RGBA8 downsample. `src_stride` is bytes per row and may include
/// padding. Allocates only the destination buffer.
pub fn downsample_rgba(
    src: &[u8],
    src_width: u32,
    src_height: u32,
    src_stride: u32,
    dst_width: u32,
    dst_height: u32,
) -> Option<Vec<u8>> {
    if src_width == 0 || src_height == 0 || dst_width == 0 || dst_height == 0 {
        return None;
    }
    let src_width = src_width as usize;
    let src_height = src_height as usize;
    let src_stride = src_stride as usize;
    let dst_width = dst_width as usize;
    let dst_height = dst_height as usize;
    let row_bytes = src_width.checked_mul(4)?;
    if src_stride < row_bytes {
        return None;
    }
    let min_len = src_height
        .checked_sub(1)?
        .checked_mul(src_stride)?
        .checked_add(row_bytes)?;
    if src.len() < min_len {
        return None;
    }

    let mut dst = vec![0u8; dst_width.checked_mul(dst_height)?.checked_mul(4)?];
    for y in 0..dst_height {
        let src_y0 = y * src_height / dst_height;
        let src_y1 = ((y + 1) * src_height / dst_height).max(src_y0 + 1);
        for x in 0..dst_width {
            let src_x0 = x * src_width / dst_width;
            let src_x1 = ((x + 1) * src_width / dst_width).max(src_x0 + 1);
            let mut sum = [0u64; 4];
            let mut count = 0u64;
            for src_y in src_y0..src_y1 {
                let row = src_y * src_stride;
                for src_x in src_x0..src_x1 {
                    let i = row + src_x * 4;
                    sum[0] += u64::from(src[i]);
                    sum[1] += u64::from(src[i + 1]);
                    sum[2] += u64::from(src[i + 2]);
                    sum[3] += u64::from(src[i + 3]);
                    count += 1;
                }
            }
            let o = (y * dst_width + x) * 4;
            if count == 0 {
                continue;
            }
            dst[o] = (sum[0] / count) as u8;
            dst[o + 1] = (sum[1] / count) as u8;
            dst[o + 2] = (sum[2] / count) as u8;
            dst[o + 3] = (sum[3] / count) as u8;
        }
    }
    Some(dst)
}

#[cfg(test)]
mod tests {
    use super::{
        capture_lod_factor, downsample_rgba, fitted_capture_size, window_capture_target,
        MAX_CAPTURE_HEIGHT, MAX_CAPTURE_WIDTH,
    };

    #[test]
    fn default_tile_requests_2x_pixels() {
        assert_eq!(window_capture_target(640.0, 360.0, 1.0, None), (1280, 736));
    }

    #[test]
    fn capture_lod_never_exceeds_100_percent() {
        assert_eq!(capture_lod_factor(1.0), 1.0);
        assert_eq!(capture_lod_factor(1.75), 1.0);
        assert_eq!(capture_lod_factor(0.3), 0.3);
        assert_eq!(capture_lod_factor(0.01), 0.05);
    }

    #[test]
    fn zoomed_out_window_capture_uses_fewer_pixels() {
        let full = window_capture_target(640.0, 360.0, 1.0, None);
        let lod = capture_lod_factor(0.3);
        let out = window_capture_target(640.0 * lod, 360.0 * lod, 1.0, None);
        assert!(out.0 < full.0);
        assert!(out.1 < full.1);
        assert_eq!(out, window_capture_target(192.0, 108.0, 1.0, None));
    }

    #[test]
    fn crop_inflates_the_full_frame_target() {
        let (width, height) =
            window_capture_target(400.0, 225.0, 1.0, Some((0.25, 0.25, 0.75, 0.75)));
        assert!(width >= 1600);
        assert!(height >= 896);
    }

    #[test]
    fn native_size_when_target_is_unset() {
        assert_eq!(fitted_capture_size(1920, 1080, 0, 0), (1920, 1080));
    }

    #[test]
    fn never_upsamples_a_smaller_window() {
        assert_eq!(fitted_capture_size(1920, 1080, 3840, 2160), (1920, 1080));
    }

    #[test]
    fn downscales_4k_into_a_small_tile_box() {
        assert_eq!(fitted_capture_size(3840, 2160, 800, 450), (800, 450));
    }

    #[test]
    fn preserves_aspect_inside_a_square_box() {
        assert_eq!(fitted_capture_size(3840, 2160, 800, 800), (800, 450));
    }

    #[test]
    fn respects_the_4k_cap() {
        assert_eq!(
            fitted_capture_size(7680, 4320, 8000, 8000),
            (MAX_CAPTURE_WIDTH, MAX_CAPTURE_HEIGHT)
        );
    }

    #[test]
    fn averages_a_solid_color() {
        let src = vec![
            200, 10, 20, 255, 200, 10, 20, 255, 200, 10, 20, 255, 200, 10, 20, 255,
        ];
        let dst = downsample_rgba(&src, 2, 2, 8, 1, 1).unwrap();
        assert_eq!(dst, vec![200, 10, 20, 255]);
    }

    #[test]
    fn averages_distinct_pixels() {
        let src = vec![0, 0, 0, 255, 40, 0, 0, 255, 0, 80, 0, 255, 0, 0, 120, 255];
        let dst = downsample_rgba(&src, 2, 2, 8, 1, 1).unwrap();
        assert_eq!(dst, vec![10, 20, 30, 255]);
    }

    #[test]
    fn skips_row_padding() {
        let mut src = vec![0u8; 2 * 16];
        src[0..4].copy_from_slice(&[10, 0, 0, 255]);
        src[4..8].copy_from_slice(&[30, 0, 0, 255]);
        src[16..20].copy_from_slice(&[10, 0, 0, 255]);
        src[20..24].copy_from_slice(&[30, 0, 0, 255]);
        let dst = downsample_rgba(&src, 2, 2, 16, 1, 1).unwrap();
        assert_eq!(dst, vec![20, 0, 0, 255]);
    }
}
