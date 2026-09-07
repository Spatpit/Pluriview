//! Resize on the capture device before transferring pixels to system memory.
//! Each worker owns its context and reuses output backing across nearby sizes.

use capture_windows::Win32::Graphics::Direct3D11::*;
use capture_windows::Win32::Graphics::Dxgi::Common::*;

const SHADER: &[u8] = include_bytes!("shaders/area_average.cso");
// Bound serial work per shader invocation and keep its integer sums exact.
// More extreme reductions continue through the existing CPU implementation.
const MAX_SAMPLES_PER_PIXEL: u32 = 4096;

pub(super) struct GpuDownsampler {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    shader: Option<ID3D11ComputeShader>,
    input_view: Option<(ID3D11Texture2D, ID3D11ShaderResourceView)>,
    resources: Option<Resources>,
    disabled: bool,
}

struct Resources {
    dimensions: [u32; 4],
    output_capacity: u32,
    shrink_since: Option<std::time::Instant>,
    output: ID3D11Buffer,
    output_view: ID3D11UnorderedAccessView,
    staging: ID3D11Buffer,
    constants: ID3D11Buffer,
}

fn eligible([width, height, out_width, out_height]: [u32; 4]) -> bool {
    width > 0
        && height > 0
        && out_width > 0
        && out_height > 0
        && width <= 16384
        && height <= 16384
        && out_width <= width
        && out_height <= height
        && (out_width < width || out_height < height)
        && width.div_ceil(out_width) * height.div_ceil(out_height) <= MAX_SAMPLES_PER_PIXEL
}

impl GpuDownsampler {
    pub fn new(device: ID3D11Device, context: ID3D11DeviceContext) -> Self {
        #[cfg(pluriview_performance)]
        let disabled = std::env::var_os("PLURIVIEW_PERF_CPU_DOWNSAMPLE").is_some();
        #[cfg(not(pluriview_performance))]
        let disabled = false;
        Self {
            device,
            context,
            shader: None,
            input_view: None,
            resources: None,
            disabled,
        }
    }

    /// Returns None when the unmodified CPU path should handle this frame.
    /// Failures disable acceleration for this worker, without failing the tile.
    pub fn downsample(
        &mut self,
        source: &ID3D11Texture2D,
        dimensions: [u32; 4],
    ) -> Option<Vec<u8>> {
        if self.disabled {
            return None;
        }
        if !eligible(dimensions) {
            self.resources = None;
            self.input_view = None;
            return None;
        }
        match self.try_downsample(source, dimensions) {
            Ok(bytes) => Some(bytes),
            Err(error) => {
                self.resources = None;
                self.shader = None;
                self.input_view = None;
                self.disabled = true;
                log::warn!("GPU capture resize unavailable; using CPU resize: {error}");
                None
            }
        }
    }

    fn try_downsample(
        &mut self,
        source: &ID3D11Texture2D,
        dimensions: [u32; 4],
    ) -> Result<Vec<u8>, String> {
        let [width, height, out_width, out_height] = dimensions;
        unsafe {
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            source.GetDesc(&mut desc);
            if desc.Format != DXGI_FORMAT_R8G8B8A8_UNORM
                || desc.BindFlags & D3D11_BIND_SHADER_RESOURCE.0 as u32 == 0
                || desc.SampleDesc.Count != 1
                || desc.ArraySize != 1
                || desc.Width < width
                || desc.Height < height
            {
                return Err("unsupported capture texture".into());
            }
            // Retain at most one surface view, never the WinRT capture frame.
            // Reusing the frame pool's texture avoids creating a view per frame;
            // a replacement surface immediately drops the previous reference.
            if self
                .input_view
                .as_ref()
                .is_none_or(|(texture, _)| texture != source)
            {
                self.input_view = None;
                let mut view = None;
                self.device
                    .CreateShaderResourceView(source, None, Some(&mut view))
                    .map_err(|e| e.to_string())?;
                self.input_view = Some((source.clone(), view.unwrap()));
                #[cfg(pluriview_performance)]
                crate::app::performance::counters::record(
                    crate::app::performance::counters::CAPTURE_GPU_VIEWS,
                );
            }
            if self.shader.is_none() {
                self.device
                    .CreateComputeShader(SHADER, None, Some(&mut self.shader))
                    .map_err(|e| e.to_string())?;
            }
            if self.resources.is_none() {
                self.resources = Some(Resources::new(&self.device, dimensions)?);
            }
            self.resources
                .as_mut()
                .unwrap()
                .resize(&self.device, &self.context, dimensions)?;
            let r = self.resources.as_ref().unwrap();
            self.context.CSSetShader(self.shader.as_ref(), None);
            self.context.CSSetShaderResources(
                0,
                Some(&[Some(self.input_view.as_ref().unwrap().1.clone())]),
            );
            self.context
                .CSSetConstantBuffers(0, Some(&[Some(r.constants.clone())]));
            self.context.CSSetUnorderedAccessViews(
                0,
                1,
                Some([Some(r.output_view.clone())].as_ptr()),
                None,
            );
            self.context
                .Dispatch(out_width.div_ceil(8), out_height.div_ceil(8), 1);
            // Unbind before copying and avoid retaining resources on the context.
            self.context
                .CSSetUnorderedAccessViews(0, 1, Some([None].as_ptr()), None);
            self.context.CSSetShaderResources(0, Some(&[None]));
            self.context.CSSetConstantBuffers(0, Some(&[None]));
            self.context.CSSetShader(None, None);
            let output_region = D3D11_BOX {
                right: out_width * out_height * 4,
                bottom: 1,
                back: 1,
                ..Default::default()
            };
            self.context.CopySubresourceRegion(
                &r.staging,
                0,
                0,
                0,
                0,
                &r.output,
                0,
                Some(&output_region),
            );

            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context
                .Map(&r.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .map_err(|e| e.to_string())?;
            let _mapping = Mapping {
                context: &self.context,
                buffer: &r.staging,
            };
            if mapped.pData.is_null() {
                return Err("invalid staging buffer".into());
            }
            Ok(std::slice::from_raw_parts(
                mapped.pData.cast::<u8>(),
                out_width as usize * out_height as usize * 4,
            )
            .to_vec())
        }
    }
}

struct Mapping<'a> {
    context: &'a ID3D11DeviceContext,
    buffer: &'a ID3D11Buffer,
}
impl Drop for Mapping<'_> {
    fn drop(&mut self) {
        unsafe { self.context.Unmap(self.buffer, 0) };
    }
}

impl Resources {
    fn capacity(width: u32, height: u32) -> u32 {
        // Geometric capacities bound allocation churn while zooming. Linear
        // buffers can reuse the same storage across aspect-ratio changes too.
        (width * height).next_power_of_two().max(16384)
    }

    fn outputs(
        device: &ID3D11Device,
        capacity: u32,
    ) -> Result<(ID3D11Buffer, ID3D11UnorderedAccessView, ID3D11Buffer), String> {
        #[cfg(pluriview_performance)]
        crate::app::performance::counters::record(
            crate::app::performance::counters::CAPTURE_GPU_BUFFERS,
        );
        unsafe {
            let desc = D3D11_BUFFER_DESC {
                ByteWidth: capacity * 4,
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_UNORDERED_ACCESS.0 as u32,
                ..Default::default()
            };
            let mut output = None;
            device
                .CreateBuffer(&desc, None, Some(&mut output))
                .map_err(|e| e.to_string())?;
            let output = output.unwrap();
            let mut view = None;
            let view_desc = D3D11_UNORDERED_ACCESS_VIEW_DESC {
                Format: DXGI_FORMAT_R32_UINT,
                ViewDimension: D3D11_UAV_DIMENSION_BUFFER,
                Anonymous: D3D11_UNORDERED_ACCESS_VIEW_DESC_0 {
                    Buffer: D3D11_BUFFER_UAV {
                        FirstElement: 0,
                        NumElements: capacity,
                        Flags: 0,
                    },
                },
            };
            device
                .CreateUnorderedAccessView(&output, Some(&view_desc), Some(&mut view))
                .map_err(|e| e.to_string())?;
            let desc = D3D11_BUFFER_DESC {
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                ..desc
            };
            let mut staging = None;
            device
                .CreateBuffer(&desc, None, Some(&mut staging))
                .map_err(|e| e.to_string())?;
            Ok((output, view.unwrap(), staging.unwrap()))
        }
    }

    fn resize(
        &mut self,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        dimensions: [u32; 4],
    ) -> Result<(), String> {
        let requested = Self::capacity(dimensions[2], dimensions[3]);
        // Grow immediately, but let a small backing request settle before
        // shrinking. Reversing a zoom should reuse the recent allocation.
        let shrink = if requested * 2 < self.output_capacity {
            if self.dimensions != dimensions {
                self.shrink_since = Some(std::time::Instant::now());
            }
            self.shrink_since
                .get_or_insert_with(std::time::Instant::now)
                .elapsed()
                >= std::time::Duration::from_secs(2)
        } else {
            self.shrink_since = None;
            false
        };
        if requested > self.output_capacity || shrink {
            let (output, view, staging) = Self::outputs(device, requested)?;
            self.output = output;
            self.output_view = view;
            self.staging = staging;
            self.output_capacity = requested;
            self.shrink_since = None;
        }
        if self.dimensions != dimensions {
            unsafe {
                context.UpdateSubresource(
                    &self.constants,
                    0,
                    None,
                    dimensions.as_ptr().cast(),
                    0,
                    0,
                );
            }
            self.dimensions = dimensions;
        }
        Ok(())
    }

    fn new(device: &ID3D11Device, dimensions: [u32; 4]) -> Result<Self, String> {
        let [_, _, out_width, out_height] = dimensions;
        unsafe {
            let output_capacity = Self::capacity(out_width, out_height);
            let (output, output_view, staging) = Self::outputs(device, output_capacity)?;
            let desc = D3D11_BUFFER_DESC {
                ByteWidth: 16,
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                ..Default::default()
            };
            let initial = D3D11_SUBRESOURCE_DATA {
                pSysMem: dimensions.as_ptr().cast(),
                ..Default::default()
            };
            let mut constants = None;
            device
                .CreateBuffer(&desc, Some(&initial), Some(&mut constants))
                .map_err(|e| e.to_string())?;
            Ok(Self {
                dimensions,
                output_capacity,
                shrink_since: None,
                output,
                output_view,
                staging,
                constants: constants.unwrap(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::downsample::RgbaDownsampler;
    use capture_windows::Win32::Foundation::HMODULE;
    use capture_windows::Win32::Graphics::Direct3D::{
        D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0,
    };

    #[test]
    fn gpu_resize_eligibility_preserves_native_and_extreme_cpu_paths() {
        assert!(eligible([3840, 2160, 800, 450]));
        assert!(!eligible([640, 360, 640, 360]));
        assert!(!eligible([640, 360, 1280, 720]));
        assert!(!eligible([3840, 2160, 1, 1]));
        assert!(!eligible([0, 360, 0, 180]));
        assert!(!eligible([u32::MAX, u32::MAX, 1, 1]));
    }

    #[test]
    #[ignore = "requires a Direct3D 11 GPU; compares real shader readbacks with CPU output"]
    fn gpu_area_average_matches_cpu_bytes_and_rebuilds_resources() {
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
            let mut gpu = GpuDownsampler::new(device.clone(), context.unwrap());
            let mut cpu = RgbaDownsampler::default();
            // Odd ratios, row padding, single-axis reductions, repeated geometry,
            // and high-contrast/transparent pixels must be byte-for-byte identical.
            for (iteration, dims) in [
                [13, 9, 7, 4],
                [64, 35, 9, 7],
                [64, 35, 9, 7],
                [1920, 1080, 320, 180],
                [1920, 1080, 301, 171],
                [1920, 1080, 640, 360],
                [1920, 1080, 128, 72],
                [129, 1, 17, 1],
                [1, 129, 1, 17],
                [64, 64, 1, 1],
            ]
            .into_iter()
            .enumerate()
            {
                let [w, h, dw, dh] = dims;
                // A capture frame pool can briefly expose a larger backing than
                // the content size during a source-window resize.
                let backing_width = w + (iteration as u32 % 2) * 7;
                let backing_height = h + (iteration as u32 % 2) * 3;
                let stride = backing_width * 4 + 16;
                let mut pixels = vec![0u8; (stride * backing_height) as usize];
                for y in 0..backing_height {
                    for x in 0..backing_width {
                        for c in 0..4 {
                            pixels[(y * stride + x * 4 + c) as usize] =
                                ((x * 197 + y * 53 + c * 71 + iteration as u32 * 37) % 256) as u8;
                        }
                    }
                }
                let desc = D3D11_TEXTURE2D_DESC {
                    Width: backing_width,
                    Height: backing_height,
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
                    SysMemPitch: stride,
                    ..Default::default()
                };
                let mut source = None;
                device
                    .CreateTexture2D(&desc, Some(&initial), Some(&mut source))
                    .unwrap();
                let source = source.unwrap();
                let previous = gpu
                    .resources
                    .as_ref()
                    .map(|r| (r.dimensions, r.output.clone(), r.output_capacity));
                let actual = gpu.downsample(&source, dims).expect("GPU path must run");
                assert_eq!(
                    actual,
                    cpu.downsample(&pixels, w, h, stride, dw, dh).unwrap(),
                    "dimensions {dims:?}"
                );
                let view = gpu.input_view.as_ref().unwrap().1.clone();
                assert_eq!(gpu.downsample(&source, dims).unwrap(), actual);
                assert_eq!(gpu.input_view.as_ref().unwrap().1, view);
                if let Some((previous_dims, output, capacity)) = previous {
                    if previous_dims[..2] == dims[..2] {
                        let resources = gpu.resources.as_ref().unwrap();
                        let requested = Resources::capacity(dw, dh);
                        if requested <= capacity {
                            assert_eq!(
                                resources.output, output,
                                "small tile resizes must reuse output backing"
                            );
                        }
                    }
                }
                if dims == [1920, 1080, 128, 72] {
                    let resources = gpu.resources.as_mut().unwrap();
                    assert!(resources.output_capacity > Resources::capacity(dw, dh));
                    resources.shrink_since =
                        Some(std::time::Instant::now() - std::time::Duration::from_secs(3));
                    assert_eq!(gpu.downsample(&source, dims).unwrap(), actual);
                    assert_eq!(
                        gpu.resources.as_ref().unwrap().output_capacity,
                        Resources::capacity(dw, dh)
                    );
                }
                if dims == [64, 64, 1, 1] {
                    assert!(gpu.downsample(&source, [64, 64, 64, 64]).is_none());
                    assert!(
                        gpu.resources.is_none(),
                        "native capture must release resize backing"
                    );
                    assert!(gpu.input_view.is_none());
                    assert!(
                        !gpu.disabled,
                        "native sizing must not disable future resizing"
                    );
                    assert!(gpu
                        .downsample(&source, [backing_width + 1, 64, 16, 16])
                        .is_none());
                    assert!(
                        gpu.disabled,
                        "invalid GPU input must fall back for the rest of the worker"
                    );
                    assert!(gpu.resources.is_none());

                    let mut unsupported = GpuDownsampler::new(device.clone(), gpu.context.clone());
                    let mut unbound = None;
                    device
                        .CreateTexture2D(
                            &D3D11_TEXTURE2D_DESC {
                                BindFlags: 0,
                                ..desc
                            },
                            Some(&initial),
                            Some(&mut unbound),
                        )
                        .unwrap();
                    assert!(unsupported.downsample(&unbound.unwrap(), dims).is_none());
                    assert!(unsupported.disabled);
                    assert!(unsupported.resources.is_none());
                }
            }
        }
    }
}
