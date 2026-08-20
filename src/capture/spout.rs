//! Receive frames from a Spout2 sender into the same RGBA capture pipeline
//! used for window tiles.

use crate::privacy;
use crate::spout::{self, share_handle_from_u32};
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::time::Duration;
use windows::core::Interface;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Resource, ID3D11Texture2D,
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ, D3D11_SDK_VERSION,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_B8G8R8A8_UNORM_SRGB,
    DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::IDXGIKeyedMutex;

use super::coordinator::CapturedFrame;
use super::downsample::{downsample_rgba, fitted_capture_size};

pub fn capture_spout_loop(
    sender_name: String,
    target_fps: Arc<AtomicU32>,
    target_width: Arc<AtomicU32>,
    target_height: Arc<AtomicU32>,
    active: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    latest_frame: Arc<Mutex<Option<CapturedFrame>>>,
    failure: Arc<Mutex<Option<String>>>,
    stop_receiver: Receiver<()>,
) {
    log::info!(
        "Receiving Spout sender {}",
        privacy::redact_title(&sender_name)
    );

    let mut receiver = match SpoutGpuReceiver::new() {
        Ok(receiver) => receiver,
        Err(error) => {
            *failure.lock() = Some(error);
            active.store(false, Ordering::Relaxed);
            return;
        }
    };

    let mut last_error: Option<String> = None;
    while active.load(Ordering::Relaxed) {
        match stop_receiver.recv_timeout(frame_interval(&target_fps)) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
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
            }
            Ok(None) => {}
            Err(error) => {
                if last_error.as_deref() != Some(error.as_str()) {
                    last_error = Some(error.clone());
                    *failure.lock() = Some(error);
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
    staging: Option<ID3D11Texture2D>,
    share_handle: u32,
    width: u32,
    height: u32,
    format: DXGI_FORMAT,
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

        Ok(Self {
            device: device.ok_or_else(|| "Direct3D 11 device was not created".to_owned())?,
            context: context.ok_or_else(|| "Direct3D 11 context was not created".to_owned())?,
            shared: None,
            staging: None,
            share_handle: 0,
            width: 0,
            height: 0,
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
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
        let staging = self
            .staging
            .as_ref()
            .ok_or_else(|| "Spout staging texture is missing".to_owned())?;

        let keyed = shared.cast::<IDXGIKeyedMutex>().ok();
        if let Some(mutex) = keyed.as_ref() {
            unsafe { mutex.AcquireSync(0, 67) }
                .map_err(|error| format!("Could not lock the Spout texture ({error})"))?;
        }
        let dst: ID3D11Resource = staging
            .cast()
            .map_err(|error| format!("Could not copy the Spout texture ({error})"))?;
        let src: ID3D11Resource = shared
            .cast()
            .map_err(|error| format!("Could not copy the Spout texture ({error})"))?;
        unsafe {
            self.context.CopyResource(&dst, &src);
        }
        if let Some(mutex) = keyed.as_ref() {
            let _ = unsafe { mutex.ReleaseSync(0) };
        }

        let staging_resource = staging
            .cast::<ID3D11Resource>()
            .map_err(|error| format!("Could not map the Spout staging texture ({error})"))?;
        let mut mapped = windows::Win32::Graphics::Direct3D11::D3D11_MAPPED_SUBRESOURCE::default();
        unsafe {
            self.context
                .Map(&staging_resource, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
        }
        .map_err(|error| format!("Could not read the Spout texture ({error})"))?;

        let packed = unsafe {
            copy_mapped_to_rgba(
                mapped.pData.cast(),
                mapped.RowPitch as usize,
                self.width,
                self.height,
                is_bgra(self.format),
            )
        };
        unsafe {
            self.context.Unmap(&staging_resource, 0);
        }

        let (out_width, out_height) =
            fitted_capture_size(self.width, self.height, target_width, target_height);
        let data = if out_width == self.width && out_height == self.height {
            packed
        } else {
            downsample_rgba(
                &packed,
                self.width,
                self.height,
                self.width * 4,
                out_width,
                out_height,
            )
            .ok_or_else(|| "Could not downscale the Spout frame".to_owned())?
        };

        Ok(Some(CapturedFrame {
            width: out_width,
            height: out_height,
            data,
        }))
    }

    fn open_shared(&mut self, info: &spout::SpoutSender) -> Result<(), String> {
        self.shared = None;
        self.staging = None;
        let handle = share_handle_from_u32(info.share_handle);
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
        let staging_desc = D3D11_TEXTURE2D_DESC {
            Width: desc.Width,
            Height: desc.Height,
            MipLevels: 1,
            ArraySize: 1,
            Format: desc.Format,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };
        let mut staging: Option<ID3D11Texture2D> = None;
        unsafe {
            self.device
                .CreateTexture2D(&staging_desc, None, Some(&mut staging))
                .map_err(|error| format!("Could not create a Spout staging texture ({error})"))?;
        }
        let staging =
            staging.ok_or_else(|| "Could not create a Spout staging texture".to_owned())?;

        if !is_8bit_rgba(desc.Format) {
            return Err(format!(
                "Spout sender '{}' uses an unsupported pixel format",
                info.name
            ));
        }

        self.share_handle = info.share_handle;
        self.width = desc.Width;
        self.height = desc.Height;
        self.format = desc.Format;
        self.shared = Some(texture);
        self.staging = Some(staging);
        Ok(())
    }
}

fn is_bgra(format: DXGI_FORMAT) -> bool {
    format == DXGI_FORMAT_B8G8R8A8_UNORM || format == DXGI_FORMAT_B8G8R8A8_UNORM_SRGB
}

fn is_8bit_rgba(format: DXGI_FORMAT) -> bool {
    is_bgra(format)
        || format == DXGI_FORMAT_R8G8B8A8_UNORM
        || format == DXGI_FORMAT_R8G8B8A8_UNORM_SRGB
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
    use super::copy_mapped_to_rgba;

    #[test]
    fn bgra_rows_swap_to_rgba_and_drop_pitch_padding() {
        let mut src = vec![0u8; 16];
        src[0..4].copy_from_slice(&[10, 20, 30, 255]); // BGRA
        src[8..12].copy_from_slice(&[40, 50, 60, 128]);
        let out = unsafe { copy_mapped_to_rgba(src.as_ptr(), 8, 1, 2, true) };
        assert_eq!(out, vec![30, 20, 10, 255, 60, 50, 40, 128]);
    }
}
