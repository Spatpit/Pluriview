#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod canvas;
mod preview;
mod capture;
mod window_picker;
mod persistence;
mod tray;

use app::PluriviewApp;
use eframe::egui;

fn main() -> eframe::Result<()> {
    env_logger::init();

    // Create the window icon (leaf)
    let icon = create_window_icon();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 720.0])
            .with_min_inner_size([800.0, 600.0])
            .with_title("Pluriview")
            .with_icon(icon),
        ..Default::default()
    };

    eframe::run_native(
        "Pluriview",
        options,
        Box::new(|cc| Ok(Box::new(PluriviewApp::new(cc)))),
    )
}

/// Create the window icon (green leaf) for title bar and taskbar
fn create_window_icon() -> egui::IconData {
    let size = 32usize;
    let mut rgba = vec![0u8; size * size * 4];

    let cx = size as f64 / 2.0;
    let cy = size as f64 / 2.0;
    let margin = size as f64 / 10.0;
    let leaf_length = size as f64 - 2.0 * margin;
    let leaf_width = leaf_length * 0.55;

    // Rotation angle (35 degrees)
    let angle = 35.0_f64.to_radians();
    let cos_a = angle.cos();
    let sin_a = angle.sin();

    // Colors
    let base_color = (107u8, 170u8, 75u8);  // Main green
    let dark_color = (70u8, 125u8, 50u8);   // Vein color

    // Draw each pixel
    for y in 0..size {
        for x in 0..size {
            let idx = (y * size + x) * 4;

            // Transform point back to unrotated space
            let px = x as f64;
            let py = y as f64;

            // Inverse rotation
            let ux = cos_a * (px - cx) + sin_a * (py - cy) + cx;
            let uy = -sin_a * (px - cx) + cos_a * (py - cy) + cy;

            // Check if point is inside leaf shape
            let t = (uy - margin) / leaf_length;

            if t >= 0.0 && t <= 1.0 {
                // Leaf width at this height
                let width_factor = (t * std::f64::consts::PI).sin().powf(0.8);
                let half_width = (leaf_width / 2.0) * width_factor;

                let dist_from_center = (ux - cx).abs();

                if dist_from_center <= half_width {
                    // Inside leaf - check if on vein
                    let on_main_vein = dist_from_center < size as f64 / 18.0 && t > 0.08 && t < 0.92;

                    // Check side veins
                    let mut on_side_vein = false;
                    for i in 1..=5 {
                        let vein_t = 0.15 + (i as f64 / 6.0) * 0.7;
                        let vein_y = margin + vein_t * leaf_length;
                        let y_dist = (uy - vein_y).abs();

                        if y_dist < size as f64 / 20.0 {
                            let vein_width_factor = (vein_t * std::f64::consts::PI).sin().powf(0.8);
                            let vein_len = (leaf_width / 2.0) * vein_width_factor * 0.75;

                            // Side veins go diagonally upward
                            let expected_x_offset = (ux - cx).abs();
                            let expected_y_offset = expected_x_offset * 0.35;
                            let actual_y = vein_y - expected_y_offset;

                            if (uy - actual_y).abs() < size as f64 / 25.0 && expected_x_offset < vein_len {
                                on_side_vein = true;
                                break;
                            }
                        }
                    }

                    let color = if on_main_vein || on_side_vein {
                        dark_color
                    } else {
                        base_color
                    };

                    rgba[idx] = color.0;     // R
                    rgba[idx + 1] = color.1; // G
                    rgba[idx + 2] = color.2; // B
                    rgba[idx + 3] = 255;     // A
                }
            }
        }
    }

    egui::IconData {
        rgba,
        width: size as u32,
        height: size as u32,
    }
}
