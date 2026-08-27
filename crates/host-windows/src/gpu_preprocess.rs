use std::mem::ManuallyDrop;
use std::sync::Arc;

use windows::Win32::Foundation::HMODULE;
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_BIND_VIDEO_ENCODER, D3D11_BOX, D3D11_CPU_ACCESS_READ,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEX2D_VPIV,
    D3D11_TEX2D_VPOV, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING,
    D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE, D3D11_VIDEO_PROCESSOR_CONTENT_DESC,
    D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_INPUT, D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_OUTPUT,
    D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0,
    D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0, D3D11_VIDEO_PROCESSOR_STREAM,
    D3D11_VIDEO_USAGE_OPTIMAL_SPEED, D3D11_VPIV_DIMENSION_TEXTURE2D, D3D11_VPOV_DIMENSION_TEXTURE2D, D3D11CreateDevice,
    ID3D11Device, ID3D11DeviceContext, ID3D11Multithread, ID3D11Texture2D, ID3D11VideoContext, ID3D11VideoContext1,
    ID3D11VideoDevice, ID3D11VideoProcessor, ID3D11VideoProcessorEnumerator,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709, DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709, DXGI_FORMAT,
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_NV12, DXGI_RATIONAL, DXGI_SAMPLE_DESC,
};
use windows::core::Interface;

const MAX_IN_FLIGHT_SURFACES: usize = 4;

/// A D3D11 texture whose lifetime is independent from the Windows Graphics
/// Capture frame pool. Keeping the device and immediate context with the
/// texture also lets preprocessing and the encoder operate on the exact WGC
/// adapter instead of guessing which GPU owns the monitor.
pub struct GpuSurface {
    pub(crate) device: ID3D11Device,
    pub(crate) context: ID3D11DeviceContext,
    pub(crate) texture: ID3D11Texture2D,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) format: DXGI_FORMAT,
}

impl GpuSurface {
    #[must_use]
    pub(crate) fn device_identity(&self) -> usize {
        Interface::as_raw(&self.device) as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SurfaceKey {
    device: usize,
    width: u32,
    height: u32,
    format: i32,
    bind_flags: u32,
}

#[derive(Default)]
pub(crate) struct GpuSurfacePool {
    key: Option<SurfaceKey>,
    surfaces: Vec<Arc<GpuSurface>>,
}

impl GpuSurfacePool {
    pub(crate) fn acquire(
        &mut self,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        width: u32,
        height: u32,
        format: DXGI_FORMAT,
        bind_flags: u32,
    ) -> Result<Option<Arc<GpuSurface>>, String> {
        let key = SurfaceKey {
            device: Interface::as_raw(device) as usize,
            width,
            height,
            format: format.0,
            bind_flags,
        };
        if self.key != Some(key) {
            self.surfaces.clear();
            self.key = Some(key);
            if let Ok(multithread) = context.cast::<ID3D11Multithread>() {
                unsafe {
                    let _ = multithread.SetMultithreadProtected(true);
                }
            }
        }
        if let Some(surface) = self.surfaces.iter().find(|surface| Arc::strong_count(surface) == 1) {
            return Ok(Some(Arc::clone(surface)));
        }
        if self.surfaces.len() >= MAX_IN_FLIGHT_SURFACES {
            return Ok(None);
        }
        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: format,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: bind_flags,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut texture = None;
        unsafe {
            device
                .CreateTexture2D(&desc, None, Some(&mut texture))
                .map_err(|error| format!("create {width}x{height} GPU video surface: {error}"))?;
        }
        let surface = Arc::new(GpuSurface {
            device: device.clone(),
            context: context.clone(),
            texture: texture.ok_or_else(|| "D3D11 returned a null video surface".to_owned())?,
            width,
            height,
            format,
        });
        self.surfaces.push(Arc::clone(&surface));
        Ok(Some(surface))
    }
}

/// Copies a WGC-owned texture into a small bounded GPU pool. This is a
/// GPU-local copy: it prevents the capture frame pool from reusing the texture
/// while another thread is preprocessing it, without reading the frame back to
/// system memory.
pub(crate) fn copy_capture_surface(
    pool: &mut GpuSurfacePool,
    device: &ID3D11Device,
    context: &ID3D11DeviceContext,
    source: &ID3D11Texture2D,
    width: u32,
    height: u32,
    format: DXGI_FORMAT,
) -> Result<Option<Arc<GpuSurface>>, String> {
    let Some(destination) = pool.acquire(device, context, width, height, format, 0)? else {
        return Ok(None);
    };
    let source_box = D3D11_BOX {
        left: 0,
        top: 0,
        front: 0,
        right: width,
        bottom: height,
        back: 1,
    };
    unsafe {
        context.CopySubresourceRegion(&destination.texture, 0, 0, 0, 0, source, 0, Some(&source_box));
    }
    Ok(Some(destination))
}

pub(crate) struct GpuVideoProcessor {
    device_identity: usize,
    input_width: u32,
    input_height: u32,
    input_format: DXGI_FORMAT,
    output_width: u32,
    output_height: u32,
    fps: u32,
    video_device: ID3D11VideoDevice,
    video_context: ID3D11VideoContext,
    enumerator: ID3D11VideoProcessorEnumerator,
    processor: ID3D11VideoProcessor,
    output_pool: GpuSurfacePool,
    output_bind_flags: u32,
    frame_index: u32,
}

impl GpuVideoProcessor {
    pub(crate) fn new(input: &GpuSurface, output_width: u32, output_height: u32, fps: u32) -> Result<Self, String> {
        let video_device = input
            .device
            .cast::<ID3D11VideoDevice>()
            .map_err(|error| format!("D3D11 video processing is unavailable on the capture adapter: {error}"))?;
        let video_context = input
            .context
            .cast::<ID3D11VideoContext>()
            .map_err(|error| format!("D3D11 video context is unavailable on the capture adapter: {error}"))?;
        let rate = DXGI_RATIONAL {
            Numerator: fps.max(1),
            Denominator: 1,
        };
        let description = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
            InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            InputFrameRate: rate,
            InputWidth: input.width,
            InputHeight: input.height,
            OutputFrameRate: rate,
            OutputWidth: output_width,
            OutputHeight: output_height,
            Usage: D3D11_VIDEO_USAGE_OPTIMAL_SPEED,
        };
        let enumerator = unsafe { video_device.CreateVideoProcessorEnumerator(&description) }
            .map_err(|error| format!("create D3D11 video processor: {error}"))?;
        let input_support = unsafe { enumerator.CheckVideoProcessorFormat(input.format) }
            .map_err(|error| format!("query capture texture support: {error}"))?;
        if input_support & D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_INPUT.0 as u32 == 0 {
            return Err(format!(
                "capture texture format {} is not a video-processor input on this adapter",
                input.format.0
            ));
        }
        let output_support = unsafe { enumerator.CheckVideoProcessorFormat(DXGI_FORMAT_NV12) }
            .map_err(|error| format!("query NV12 output support: {error}"))?;
        if output_support & D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_OUTPUT.0 as u32 == 0 {
            return Err("the capture adapter cannot produce NV12 video-processor output".to_owned());
        }
        let processor = unsafe { video_device.CreateVideoProcessor(&enumerator, 0) }
            .map_err(|error| format!("create low-latency D3D11 video processor: {error}"))?;
        Ok(Self {
            device_identity: input.device_identity(),
            input_width: input.width,
            input_height: input.height,
            input_format: input.format,
            output_width,
            output_height,
            fps,
            video_device,
            video_context,
            enumerator,
            processor,
            output_pool: GpuSurfacePool::default(),
            output_bind_flags: D3D11_BIND_RENDER_TARGET.0 as u32
                | D3D11_BIND_VIDEO_ENCODER.0 as u32
                | D3D11_BIND_SHADER_RESOURCE.0 as u32,
            frame_index: 0,
        })
    }

    #[must_use]
    pub(crate) fn matches(&self, input: &GpuSurface, output_width: u32, output_height: u32, fps: u32) -> bool {
        self.device_identity == input.device_identity()
            && self.input_width == input.width
            && self.input_height == input.height
            && self.input_format == input.format
            && self.output_width == output_width
            && self.output_height == output_height
            && self.fps == fps
    }

    pub(crate) fn process(&mut self, input: &GpuSurface) -> Result<Option<Arc<GpuSurface>>, String> {
        let output = match self.output_pool.acquire(
            &input.device,
            &input.context,
            self.output_width,
            self.output_height,
            DXGI_FORMAT_NV12,
            self.output_bind_flags,
        ) {
            Ok(output) => output,
            Err(first_error) if self.output_bind_flags != D3D11_BIND_RENDER_TARGET.0 as u32 => {
                self.output_bind_flags = D3D11_BIND_RENDER_TARGET.0 as u32;
                self.output_pool
                    .acquire(
                        &input.device,
                        &input.context,
                        self.output_width,
                        self.output_height,
                        DXGI_FORMAT_NV12,
                        self.output_bind_flags,
                    )
                    .map_err(|fallback_error| {
                        format!(
                            "create encoder-ready NV12 surface: {first_error}; render-target fallback: {fallback_error}"
                        )
                    })?
            }
            Err(error) => return Err(error),
        };
        let Some(output) = output else {
            return Ok(None);
        };
        let input_description = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
            FourCC: 0,
            ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_VPIV {
                    MipSlice: 0,
                    ArraySlice: 0,
                },
            },
        };
        let mut input_view = None;
        unsafe {
            self.video_device
                .CreateVideoProcessorInputView(
                    &input.texture,
                    &self.enumerator,
                    &input_description,
                    Some(&mut input_view),
                )
                .map_err(|error| format!("create BGRA video-processor input view: {error}"))?;
        }
        let output_description = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
            ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 },
            },
        };
        let mut output_view = None;
        unsafe {
            self.video_device
                .CreateVideoProcessorOutputView(
                    &output.texture,
                    &self.enumerator,
                    &output_description,
                    Some(&mut output_view),
                )
                .map_err(|error| format!("create NV12 video-processor output view: {error}"))?;
        }
        let input_view = input_view.ok_or_else(|| "D3D11 returned a null input view".to_owned())?;
        let output_view = output_view.ok_or_else(|| "D3D11 returned a null output view".to_owned())?;
        let source_rect = RECT {
            left: 0,
            top: 0,
            right: input.width as i32,
            bottom: input.height as i32,
        };
        let destination_rect = RECT {
            left: 0,
            top: 0,
            right: self.output_width as i32,
            bottom: self.output_height as i32,
        };
        unsafe {
            self.video_context.VideoProcessorSetStreamFrameFormat(
                &self.processor,
                0,
                D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            );
            self.video_context
                .VideoProcessorSetStreamSourceRect(&self.processor, 0, true, Some(&source_rect));
            self.video_context
                .VideoProcessorSetStreamDestRect(&self.processor, 0, true, Some(&destination_rect));
            if let Ok(context1) = self.video_context.cast::<ID3D11VideoContext1>() {
                context1.VideoProcessorSetStreamColorSpace1(
                    &self.processor,
                    0,
                    DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709,
                );
                context1
                    .VideoProcessorSetOutputColorSpace1(&self.processor, DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709);
            }
        }
        let mut stream = D3D11_VIDEO_PROCESSOR_STREAM {
            Enable: true.into(),
            OutputIndex: 0,
            InputFrameOrField: 0,
            PastFrames: 0,
            FutureFrames: 0,
            ppPastSurfaces: std::ptr::null_mut(),
            pInputSurface: ManuallyDrop::new(Some(input_view)),
            ppFutureSurfaces: std::ptr::null_mut(),
            ppPastSurfacesRight: std::ptr::null_mut(),
            pInputSurfaceRight: ManuallyDrop::new(None),
            ppFutureSurfacesRight: std::ptr::null_mut(),
        };
        let result = unsafe {
            self.video_context.VideoProcessorBlt(
                &self.processor,
                &output_view,
                self.frame_index,
                std::slice::from_ref(&stream),
            )
        };
        unsafe {
            drop(ManuallyDrop::take(&mut stream.pInputSurface));
            drop(ManuallyDrop::take(&mut stream.pInputSurfaceRight));
        }
        result.map_err(|error| format!("GPU resize/BGRA-to-NV12 conversion failed: {error}"))?;
        unsafe {
            input.context.Flush();
        }
        self.frame_index = self.frame_index.wrapping_add(1);
        Ok(Some(output))
    }
}

/// Deterministic host benchmarks have CPU-rendered source patterns rather than
/// a WGC texture. This adapter uploads that known BGRA frame to a persistent
/// texture, then exercises the same GPU resize/conversion and MF surface path
/// as live monitor capture. The upload is intentionally included in benchmark
/// preprocessing time and is identified in the report.
pub(crate) struct GpuBenchmarkPipeline {
    input: Arc<GpuSurface>,
    processor: GpuVideoProcessor,
}

impl GpuBenchmarkPipeline {
    pub(crate) fn new(
        input_width: u32,
        input_height: u32,
        output_width: u32,
        output_height: u32,
        fps: u32,
    ) -> Result<Self, String> {
        let feature_levels = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0];
        let mut device = None;
        let mut context = None;
        let mut selected_level = D3D_FEATURE_LEVEL::default();
        unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&feature_levels),
                D3D11_SDK_VERSION,
                Some(&mut device),
                Some(&mut selected_level),
                Some(&mut context),
            )
            .map_err(|error| format!("create hardware D3D11 benchmark device: {error}"))?;
        }
        if selected_level.0 < D3D_FEATURE_LEVEL_11_0.0 {
            return Err("the benchmark adapter does not support D3D feature level 11.0".to_owned());
        }
        let device = device.ok_or_else(|| "D3D11 returned a null benchmark device".to_owned())?;
        let context = context.ok_or_else(|| "D3D11 returned a null benchmark context".to_owned())?;
        let mut pool = GpuSurfacePool::default();
        let input = pool
            .acquire(
                &device,
                &context,
                input_width,
                input_height,
                DXGI_FORMAT_B8G8R8A8_UNORM,
                0,
            )?
            .ok_or_else(|| "the GPU benchmark input surface pool is busy".to_owned())?;
        let processor = GpuVideoProcessor::new(&input, output_width, output_height, fps)?;
        Ok(Self { input, processor })
    }

    pub(crate) fn process_bgra(&mut self, bgra: &[u8]) -> Result<Arc<GpuSurface>, String> {
        let expected = self.input.width as usize * self.input.height as usize * 4;
        if bgra.len() != expected {
            return Err(format!(
                "GPU benchmark frame has {} bytes; expected {expected}",
                bgra.len()
            ));
        }
        unsafe {
            self.input.context.UpdateSubresource(
                &self.input.texture,
                0,
                None,
                bgra.as_ptr().cast(),
                self.input.width * 4,
                0,
            );
        }
        self.processor
            .process(&self.input)?
            .ok_or_else(|| "the bounded GPU benchmark output pool is busy".to_owned())
    }
}

pub(crate) fn read_bgra(surface: &GpuSurface) -> Result<Vec<u8>, String> {
    if surface.format != DXGI_FORMAT_B8G8R8A8_UNORM {
        return Err(format!(
            "cannot read unsupported capture format {} as BGRA",
            surface.format.0
        ));
    }
    let staging = create_staging_texture(surface, surface.format)?;
    let mapped = copy_and_map(surface, &staging)?;
    let row_bytes = surface.width as usize * 4;
    let mut output = Vec::with_capacity(row_bytes * surface.height as usize);
    unsafe {
        let source = mapped.pData.cast::<u8>();
        for row in 0..surface.height as usize {
            output.extend_from_slice(std::slice::from_raw_parts(
                source.add(row * mapped.RowPitch as usize),
                row_bytes,
            ));
        }
        surface.context.Unmap(&staging, 0);
    }
    Ok(output)
}

#[derive(Default)]
pub(crate) struct Nv12Readback {
    key: Option<(usize, u32, u32)>,
    staging: Option<ID3D11Texture2D>,
}

impl Nv12Readback {
    pub(crate) fn read(&mut self, surface: &GpuSurface) -> Result<Vec<u8>, String> {
        if surface.format != DXGI_FORMAT_NV12 {
            return Err(format!("cannot read format {} as NV12", surface.format.0));
        }
        let key = (surface.device_identity(), surface.width, surface.height);
        if self.key != Some(key) {
            self.staging = Some(create_staging_texture(surface, DXGI_FORMAT_NV12)?);
            self.key = Some(key);
        }
        let staging = self
            .staging
            .as_ref()
            .ok_or_else(|| "NV12 staging surface was not created".to_owned())?;
        let mapped = copy_and_map(surface, staging)?;
        let row_bytes = surface.width as usize;
        let mut output = Vec::with_capacity(row_bytes * surface.height as usize * 3 / 2);
        unsafe {
            let source = mapped.pData.cast::<u8>();
            for row in 0..surface.height as usize {
                output.extend_from_slice(std::slice::from_raw_parts(
                    source.add(row * mapped.RowPitch as usize),
                    row_bytes,
                ));
            }
            let uv_start = source.add(mapped.RowPitch as usize * surface.height as usize);
            for row in 0..surface.height as usize / 2 {
                output.extend_from_slice(std::slice::from_raw_parts(
                    uv_start.add(row * mapped.RowPitch as usize),
                    row_bytes,
                ));
            }
            surface.context.Unmap(staging, 0);
        }
        Ok(output)
    }
}

fn create_staging_texture(surface: &GpuSurface, format: DXGI_FORMAT) -> Result<ID3D11Texture2D, String> {
    let description = D3D11_TEXTURE2D_DESC {
        Width: surface.width,
        Height: surface.height,
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        MiscFlags: 0,
    };
    let mut staging = None;
    unsafe {
        surface
            .device
            .CreateTexture2D(&description, None, Some(&mut staging))
            .map_err(|error| format!("create GPU readback surface: {error}"))?;
    }
    staging.ok_or_else(|| "D3D11 returned a null readback surface".to_owned())
}

fn copy_and_map(surface: &GpuSurface, staging: &ID3D11Texture2D) -> Result<D3D11_MAPPED_SUBRESOURCE, String> {
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    unsafe {
        surface.context.CopyResource(staging, &surface.texture);
        surface
            .context
            .Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
            .map_err(|error| format!("map GPU readback surface: {error}"))?;
    }
    Ok(mapped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surface_pool_is_strictly_bounded() {
        assert_eq!(MAX_IN_FLIGHT_SURFACES, 4);
    }
}
