//! Receive frames from a Spout2 sender into the same RGBA capture pipeline
//! used for window tiles.

// The same Direct3D bindings as the window capture workers, so Spout frames
// go through the shared GPU resize and readback.
use crate::privacy;
use crate::spout::{self, share_handle_from_u32};
use capture_windows::core::Interface;
use capture_windows::Win32::Foundation::{HANDLE, HMODULE};
use capture_windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0,
};
use capture_windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
    D3D11_BIND_SHADER_RESOURCE, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
};
use capture_windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_B8G8R8A8_UNORM_SRGB,
    DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB, DXGI_SAMPLE_DESC,
};
use capture_windows::Win32::Graphics::Dxgi::IDXGIKeyedMutex;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Duration;

use super::coordinator::{CaptureWorkerState, CapturedFrame, WorkerSignal};
use super::downsample::{fitted_capture_size, RgbaDownsampler};
use super::gpu_downsample::GpuDownsampler;
use super::readback::StagingReadback;

pub fn capture_spout_loop(
    sender_name: String,
    worker_state: CaptureWorkerState,
    stop_receiver: Receiver<WorkerSignal>,
) {
    let CaptureWorkerState {
        target_fps,
        target_width,
        target_height,
        target_generation: _,
        active,
        paused,
        has_produced_frame,
        latest_frame,
        failure,
        frame_wake,
    } = worker_state;
    log::info!(
        "Receiving Spout sender {}",
        privacy::redact_title(&sender_name)
    );

    let mut receiver = match SpoutGpuReceiver::new() {
        Ok(receiver) => receiver,
        Err(error) => {
            *failure.lock() = Some(error);
            active.store(false, Ordering::Relaxed);
            frame_wake.notify();
            return;
        }
    };

    let mut last_error: Option<String> = None;
    while active.load(Ordering::Relaxed) {
        match stop_receiver.recv_timeout(frame_interval(&target_fps)) {
            Ok(WorkerSignal::Stop) | Err(RecvTimeoutError::Disconnected) => break,
            // Spout sessions are never retargeted; wait for the next tick.
            Ok(WorkerSignal::Retarget) => continue,
            Err(RecvTimeoutError::Timeout) => {}
        }
        if !active.load(Ordering::Relaxed) {
            break;
        }
        if paused.load(Ordering::Relaxed) {
            continue;
        }

        match receiver.grab(
            &sender_name,
            target_width.load(Ordering::Relaxed),
            target_height.load(Ordering::Relaxed),
        ) {
            Ok(Some(frame)) => {
                last_error = None;
                *latest_frame.lock() = Some(frame);
                has_produced_frame.store(true, Ordering::Relaxed);
                frame_wake.notify();
            }
            Ok(None) => {}
            Err(error) => {
                if last_error.as_deref() != Some(error.as_str()) {
                    last_error = Some(error.clone());
                    *failure.lock() = Some(error);
                    frame_wake.notify();
                }
            }
        }
    }

    active.store(false, Ordering::Relaxed);
}

fn frame_interval(target_fps: &AtomicU32) -> Duration {
    let fps = target_fps.load(Ordering::Relaxed).max(1) as f64;
    Duration::from_secs_f64(1.0 / fps)
}

struct SpoutGpuReceiver {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    shared: Option<ID3D11Texture2D>,
    /// This worker's copy of the sender's frame, taken while holding the
    /// sender's lock. Its format has no sRGB conversion, so reading it gives
    /// the sender's bytes, as the CPU path always did.
    copy: Option<ID3D11Texture2D>,
    share_handle: u32,
    width: u32,
    height: u32,
    bgra: bool,
    readback: StagingReadback,
    gpu_downsampler: GpuDownsampler,
    downsampler: RgbaDownsampler,
}

impl SpoutGpuReceiver {
    fn new() -> Result<Self, String> {
        let mut device = None;
        let mut context = None;
        let levels = [D3D_FEATURE_LEVEL_11_0];
        unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&levels),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
        }
        .map_err(|error| format!("Could not create a Direct3D 11 device ({error})"))?;
        let device: ID3D11Device =
            device.ok_or_else(|| "Direct3D 11 device was not created".to_owned())?;
        let context: ID3D11DeviceContext =
            context.ok_or_else(|| "Direct3D 11 context was not created".to_owned())?;

        Ok(Self {
            readback: StagingReadback::new(device.clone(), context.clone()),
            gpu_downsampler: GpuDownsampler::new(device.clone(), context.clone()),
            device,
            context,
            shared: None,
            copy: None,
            share_handle: 0,
            width: 0,
            height: 0,
            bgra: true,
            downsampler: RgbaDownsampler::default(),
        })
    }

    fn grab(
        &mut self,
        sender_name: &str,
        target_width: u32,
        target_height: u32,
    ) -> Result<Option<CapturedFrame>, String> {
        let info = spout::sender_info(sender_name)
            .ok_or_else(|| format!("Waiting for Spout sender '{sender_name}'"))?;
        if info.width == 0 || info.height == 0 {
            return Err(format!("Spout sender '{sender_name}' has no size yet"));
        }
        if info.share_handle == 0 {
            return Err(format!(
                "Spout sender '{sender_name}' is not sharing a GPU texture"
            ));
        }

        if self.shared.is_none()
            || self.share_handle != info.share_handle
            || self.width != info.width
            || self.height != info.height
        {
            self.open_shared(&info)?;
        }

        let shared = self
            .shared
            .as_ref()
            .ok_or_else(|| "Spout shared texture is missing".to_owned())?;
        let copy = self
            .copy
            .clone()
            .ok_or_else(|| "Spout copy texture is missing".to_owned())?;

        let keyed = shared.cast::<IDXGIKeyedMutex>().ok();
        if let Some(mutex) = keyed.as_ref() {
            unsafe { mutex.AcquireSync(0, 67) }
                .map_err(|error| format!("Could not lock the Spout texture ({error})"))?;
        }
        unsafe {
            self.context.CopyResource(&copy, shared);
        }
        if let Some(mutex) = keyed.as_ref() {
            let _ = unsafe { mutex.ReleaseSync(0) };
        }
        self.frame_from_copy(&copy, target_width, target_height)
            .map(Some)
    }

    /// The tile's frame from this worker's copy of the sender's texture.
    fn frame_from_copy(
        &mut self,
        copy: &ID3D11Texture2D,
        target_width: u32,
        target_height: u32,
    ) -> Result<CapturedFrame, String> {
        let (width, height, bgra) = (self.width, self.height, self.bgra);
        let (out_width, out_height) =
            fitted_capture_size(width, height, target_width, target_height);
        if out_width == width && out_height == height {
            return self
                .readback
                .read(copy, width, height, |rows, row_pitch| {
                    let rgba = unsafe {
                        copy_mapped_to_rgba(rows.as_ptr(), row_pitch as usize, width, height, bgra)
                    };
                    CapturedFrame::from_rgba(width, height, width, height, &rgba)
                })
                .map_err(|error| format!("Could not read the Spout texture ({error})"));
        }

        // Resize on the GPU and read back only the tile-sized result. The
        // shader reads channels by name, so it returns RGBA for BGRA too.
        let data = match self
            .gpu_downsampler
            .downsample(copy, [width, height, out_width, out_height])
        {
            Some(data) => data,
            None => {
                let downsampler = &mut self.downsampler;
                let mut data = self
                    .readback
                    .read(copy, width, height, |rows, row_pitch| {
                        downsampler
                            .downsample(rows, width, height, row_pitch, out_width, out_height)
                    })
                    .map_err(|error| format!("Could not read the Spout texture ({error})"))?
                    .ok_or_else(|| "Could not downscale the Spout frame".to_owned())?;
                if bgra {
                    swap_red_blue(&mut data);
                }
                data
            }
        };
        Ok(CapturedFrame::from_rgba(
            out_width, out_height, width, height, &data,
        ))
    }

    fn open_shared(&mut self, info: &spout::SpoutSender) -> Result<(), String> {
        self.shared = None;
        self.copy = None;
        let handle = HANDLE(share_handle_from_u32(info.share_handle).0);
        let mut texture: Option<ID3D11Texture2D> = None;
        unsafe {
            self.device
                .OpenSharedResource(handle, &mut texture)
                .map_err(|error| {
                    format!(
                        "Could not open Spout sender '{}'. Use the same GPU as the app. ({error})",
                        info.name
                    )
                })?;
        }
        let texture = texture.ok_or_else(|| {
            format!(
                "Could not open Spout sender '{}'. Use the same GPU as the app.",
                info.name
            )
        })?;

        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { texture.GetDesc(&mut desc) };
        let Some(copy_format) = raw_copy_format(desc.Format) else {
            return Err(format!(
                "Spout sender '{}' uses an unsupported pixel format",
                info.name
            ));
        };
        let copy_desc = D3D11_TEXTURE2D_DESC {
            Width: desc.Width,
            Height: desc.Height,
            MipLevels: 1,
            ArraySize: 1,
            Format: copy_format,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut copy: Option<ID3D11Texture2D> = None;
        unsafe {
            self.device
                .CreateTexture2D(&copy_desc, None, Some(&mut copy))
                .map_err(|error| format!("Could not create a Spout copy texture ({error})"))?;
        }
        let copy = copy.ok_or_else(|| "Could not create a Spout copy texture".to_owned())?;

        self.share_handle = info.share_handle;
        self.width = desc.Width;
        self.height = desc.Height;
        self.bgra = is_bgra(desc.Format);
        self.shared = Some(texture);
        self.copy = Some(copy);
        Ok(())
    }
}

fn is_bgra(format: DXGI_FORMAT) -> bool {
    format == DXGI_FORMAT_B8G8R8A8_UNORM || format == DXGI_FORMAT_B8G8R8A8_UNORM_SRGB
}

/// The format a copy of a sender's texture uses: the same bytes without
/// sRGB conversion (a copy may change format within one type group), or
/// `None` for a format Pluriview cannot read.
fn raw_copy_format(format: DXGI_FORMAT) -> Option<DXGI_FORMAT> {
    if is_bgra(format) {
        Some(DXGI_FORMAT_B8G8R8A8_UNORM)
    } else if format == DXGI_FORMAT_R8G8B8A8_UNORM || format == DXGI_FORMAT_R8G8B8A8_UNORM_SRGB {
        Some(DXGI_FORMAT_R8G8B8A8_UNORM)
    } else {
        None
    }
}

fn swap_red_blue(rgba: &mut [u8]) {
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
}

unsafe fn copy_mapped_to_rgba(
    src: *const u8,
    row_pitch: usize,
    width: u32,
    height: u32,
    bgra: bool,
) -> Vec<u8> {
    let width = width as usize;
    let height = height as usize;
    let row_bytes = width * 4;
    let mut out = vec![0u8; row_bytes * height];
    for y in 0..height {
        let src_row = src.add(y * row_pitch);
        let dst_row = y * row_bytes;
        if bgra {
            for x in 0..width {
                let i = x * 4;
                let s = src_row.add(i);
                out[dst_row + i] = *s.add(2);
                out[dst_row + i + 1] = *s.add(1);
                out[dst_row + i + 2] = *s;
                out[dst_row + i + 3] = *s.add(3);
            }
        } else {
            std::ptr::copy_nonoverlapping(src_row, out[dst_row..].as_mut_ptr(), row_bytes);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{copy_mapped_to_rgba, swap_red_blue, SpoutGpuReceiver};
    use crate::capture::downsample::RgbaDownsampler;
    use capture_windows::Win32::Graphics::Direct3D11::{
        D3D11_BIND_SHADER_RESOURCE, D3D11_SUBRESOURCE_DATA, D3D11_TEXTURE2D_DESC,
        D3D11_USAGE_DEFAULT,
    };
    use capture_windows::Win32::Graphics::Dxgi::Common::{
        DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
    };

    #[test]
    #[ignore = "requires a Direct3D 11 GPU; reads a BGRA texture as a Spout copy"]
    fn spout_frames_resize_on_the_gpu_and_come_out_rgba() {
        let mut receiver = SpoutGpuReceiver::new().unwrap();
        let (width, height) = (97_u32, 41_u32);
        // Opaque pixels, so egui's premultiplied frame keeps the exact bytes.
        let bgra: Vec<u8> = (0..width * height)
            .flat_map(|index| {
                [
                    (index * 7 % 256) as u8,
                    (index * 13 % 256) as u8,
                    (index * 29 % 256) as u8,
                    255,
                ]
            })
            .collect();
        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
            ..Default::default()
        };
        let initial = D3D11_SUBRESOURCE_DATA {
            pSysMem: bgra.as_ptr().cast(),
            SysMemPitch: width * 4,
            ..Default::default()
        };
        let mut copy = None;
        unsafe {
            receiver
                .device
                .CreateTexture2D(&desc, Some(&initial), Some(&mut copy))
                .unwrap();
        }
        let copy = copy.unwrap();
        receiver.width = width;
        receiver.height = height;
        receiver.bgra = true;
        let bytes = |frame: &super::CapturedFrame| -> Vec<u8> {
            frame
                .image
                .pixels
                .iter()
                .flat_map(|pixel| pixel.to_array())
                .collect()
        };

        let mut rgba = bgra.clone();
        swap_red_blue(&mut rgba);
        let native = receiver.frame_from_copy(&copy, 0, 0).unwrap();
        assert_eq!(native.image.size, [width as usize, height as usize]);
        assert_eq!(bytes(&native), rgba);

        let small = receiver.frame_from_copy(&copy, 48, 20).unwrap();
        let [out_width, out_height] = small.image.size;
        let mut expected = RgbaDownsampler::default()
            .downsample(
                &bgra,
                width,
                height,
                width * 4,
                out_width as u32,
                out_height as u32,
            )
            .unwrap();
        swap_red_blue(&mut expected);
        assert_eq!(bytes(&small), expected);
        assert!(
            !receiver.gpu_downsampler.is_disabled(),
            "the resize must run on the GPU"
        );
    }

    #[test]
    fn bgra_rows_swap_to_rgba_and_drop_pitch_padding() {
        let mut src = [0u8; 16];
        src[0..4].copy_from_slice(&[10, 20, 30, 255]); // BGRA
        src[8..12].copy_from_slice(&[40, 50, 60, 128]);
        let out = unsafe { copy_mapped_to_rgba(src.as_ptr(), 8, 1, 2, true) };
        assert_eq!(out, vec![30, 20, 10, 255, 60, 50, 40, 128]);
    }

    #[test]
    fn swaps_only_red_and_blue_channels() {
        let mut pixels = vec![10, 20, 30, 40, 50, 60, 70, 80];
        swap_red_blue(&mut pixels);
        assert_eq!(pixels, [30, 20, 10, 40, 70, 60, 50, 80]);
    }
}
