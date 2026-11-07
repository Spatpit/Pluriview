//! Copies capture frames into CPU memory through one staging texture per
//! worker. windows-capture's `Frame::buffer` creates a new staging texture on
//! every call, which costs a driver allocation of up to 33 MB per frame for a
//! window shown at its native size.

use capture_windows::Win32::Graphics::Direct3D11::*;
use capture_windows::Win32::Graphics::Dxgi::Common::*;

pub(super) struct StagingReadback {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    staging: Option<Staging>,
}

struct Staging {
    texture: ID3D11Texture2D,
    width: u32,
    height: u32,
    format: DXGI_FORMAT,
}

impl StagingReadback {
    pub fn new(device: ID3D11Device, context: ID3D11DeviceContext) -> Self {
        Self {
            device,
            context,
            staging: None,
        }
    }

    /// Calls `read` with the top-left `width` x `height` pixels of `source`
    /// and their row pitch in bytes. The slice ends with the last pixel of
    /// the last row, so it holds `(height - 1) * row_pitch + width * 4` bytes.
    /// `source` is RGBA or BGRA; the bytes keep its channel order.
    pub fn read<R>(
        &mut self,
        source: &ID3D11Texture2D,
        width: u32,
        height: u32,
        read: impl FnOnce(&[u8], u32) -> R,
    ) -> Result<R, String> {
        unsafe {
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            source.GetDesc(&mut desc);
            if (desc.Format != DXGI_FORMAT_R8G8B8A8_UNORM
                && desc.Format != DXGI_FORMAT_B8G8R8A8_UNORM)
                || desc.SampleDesc.Count != 1
                || width == 0
                || height == 0
                || desc.Width < width
                || desc.Height < height
            {
                return Err("unsupported capture texture".into());
            }
            if self.staging.as_ref().is_none_or(|staging| {
                (staging.width, staging.height, staging.format) != (width, height, desc.Format)
            }) {
                self.staging = None;
                let staging_desc = D3D11_TEXTURE2D_DESC {
                    Width: width,
                    Height: height,
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
                let mut texture = None;
                self.device
                    .CreateTexture2D(&staging_desc, None, Some(&mut texture))
                    .map_err(|error| error.to_string())?;
                self.staging = Some(Staging {
                    texture: texture.ok_or("no staging texture")?,
                    width,
                    height,
                    format: desc.Format,
                });
            }
            let staging = self.staging.as_ref().unwrap();
            let content = D3D11_BOX {
                right: width,
                bottom: height,
                back: 1,
                ..Default::default()
            };
            self.context.CopySubresourceRegion(
                &staging.texture,
                0,
                0,
                0,
                0,
                source,
                0,
                Some(&content),
            );
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            if let Err(error) =
                self.context
                    .Map(&staging.texture, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
            {
                // A lost device fails every later copy too; start over.
                self.staging = None;
                return Err(error.to_string());
            }
            let _mapping = Mapping {
                context: &self.context,
                texture: &staging.texture,
            };
            let row_bytes = width as usize * 4;
            let row_pitch = mapped.RowPitch;
            if mapped.pData.is_null() || (row_pitch as usize) < row_bytes {
                return Err("invalid staging texture mapping".into());
            }
            let len = (height as usize - 1) * row_pitch as usize + row_bytes;
            let pixels = std::slice::from_raw_parts(mapped.pData.cast::<u8>(), len);
            Ok(read(pixels, row_pitch))
        }
    }
}

struct Mapping<'a> {
    context: &'a ID3D11DeviceContext,
    texture: &'a ID3D11Texture2D,
}

impl Drop for Mapping<'_> {
    fn drop(&mut self) {
        unsafe { self.context.Unmap(self.texture, 0) };
    }
}

/// `width` x `height` RGBA pixels from rows `row_pitch` bytes apart, without
/// the padding between them. Unpadded rows are returned as they are; padded
/// ones are packed into `scratch`, which keeps its storage between frames.
pub(super) fn unpadded_rgba<'a>(
    rows: &'a [u8],
    width: u32,
    height: u32,
    row_pitch: u32,
    scratch: &'a mut Vec<u8>,
) -> &'a [u8] {
    let row_bytes = width as usize * 4;
    let size = row_bytes * height as usize;
    if row_pitch as usize == row_bytes {
        return &rows[..size];
    }
    scratch.resize(size, 0);
    for (row, packed) in scratch.chunks_exact_mut(row_bytes).enumerate() {
        let start = row * row_pitch as usize;
        packed.copy_from_slice(&rows[start..start + row_bytes]);
    }
    scratch
}

#[cfg(test)]
mod tests {
    use super::*;
    use capture_windows::Win32::Foundation::HMODULE;
    use capture_windows::Win32::Graphics::Direct3D::{
        D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0,
    };

    #[test]
    fn padded_rows_unpack_without_stale_pixels_across_size_changes() {
        let mut scratch = Vec::new();
        let mut largest_capacity = 0;
        // Grow, shrink, then reuse the original size. Padding bytes must never
        // become pixels, and shrinking must not append stale rows to the frame.
        for (width, height, padding) in [(7, 5, 12), (3, 2, 4), (7, 5, 12), (4, 3, 0)] {
            let stride = width * 4 + padding;
            // The mapped slice ends at the last pixel, without the last padding.
            let mut raw = vec![0xee; (stride * (height - 1) + width * 4) as usize];
            let mut expected = Vec::new();
            for y in 0..height {
                for x in 0..width {
                    let pixel = [x as u8, y as u8, (x + y) as u8, (x * y) as u8];
                    let start = (y * stride + x * 4) as usize;
                    raw[start..start + 4].copy_from_slice(&pixel);
                    expected.extend_from_slice(&pixel);
                }
            }
            assert_eq!(
                unpadded_rgba(&raw, width, height, stride, &mut scratch),
                expected
            );
            if largest_capacity == 0 {
                largest_capacity = scratch.capacity();
            } else {
                assert_eq!(scratch.capacity(), largest_capacity);
            }
        }
    }

    #[test]
    fn unpadded_rows_are_used_in_place() {
        let raw = [1, 2, 3, 0, 5, 6, 7, 255];
        let mut scratch = Vec::new();
        let pixels = unpadded_rgba(&raw, 2, 1, 8, &mut scratch);
        assert_eq!(pixels, raw);
        assert_eq!(pixels.as_ptr(), raw.as_ptr());
        assert_eq!(scratch.capacity(), 0);
    }

    #[test]
    #[ignore = "requires a Direct3D 11 GPU; reads real textures back through one staging texture"]
    fn readback_reuses_its_staging_texture_until_the_size_changes() {
        unsafe {
            let mut device = None;
            let mut context = None;
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
            .unwrap();
            let device = device.unwrap();
            let mut readback = StagingReadback::new(device.clone(), context.unwrap());
            let texture = |width: u32, height: u32, seed: u32| {
                let pixels: Vec<u8> = (0..width * height * 4)
                    .map(|index| ((index + seed) * 31 % 251) as u8)
                    .collect();
                let desc = D3D11_TEXTURE2D_DESC {
                    Width: width,
                    Height: height,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                    SampleDesc: DXGI_SAMPLE_DESC {
                        Count: 1,
                        Quality: 0,
                    },
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                    ..Default::default()
                };
                let initial = D3D11_SUBRESOURCE_DATA {
                    pSysMem: pixels.as_ptr().cast(),
                    SysMemPitch: width * 4,
                    ..Default::default()
                };
                let mut source = None;
                device
                    .CreateTexture2D(&desc, Some(&initial), Some(&mut source))
                    .unwrap();
                (source.unwrap(), pixels)
            };
            let content = |pixels: &[u8], stride: u32, width: u32, height: u32| -> Vec<u8> {
                (0..height)
                    .flat_map(|y| {
                        let row = (y * stride) as usize;
                        pixels[row..row + (width * 4) as usize].to_vec()
                    })
                    .collect()
            };
            let mut scratch = Vec::new();
            let mut read = |readback: &mut StagingReadback, source, width, height| {
                readback
                    .read(source, width, height, |rows, pitch| {
                        unpadded_rgba(rows, width, height, pitch, &mut scratch).to_vec()
                    })
                    .unwrap()
            };

            // An odd width gives the staging texture a padded row pitch.
            let (first, first_pixels) = texture(97, 41, 0);
            assert_eq!(
                read(&mut readback, &first, 97, 41),
                content(&first_pixels, 97 * 4, 97, 41)
            );
            let kept = readback.staging.as_ref().unwrap().texture.clone();
            let (second, second_pixels) = texture(97, 41, 7);
            assert_eq!(
                read(&mut readback, &second, 97, 41),
                content(&second_pixels, 97 * 4, 97, 41)
            );
            assert_eq!(
                readback.staging.as_ref().unwrap().texture,
                kept,
                "frames of the same size must reuse the staging texture"
            );

            // Content smaller than the frame pool's texture reads only the content.
            assert_eq!(
                read(&mut readback, &second, 90, 40),
                content(&second_pixels, 97 * 4, 90, 40)
            );
            assert_ne!(readback.staging.as_ref().unwrap().texture, kept);
            assert!(readback.read(&second, 98, 41, |_, _| ()).is_err());
        }
    }
}
