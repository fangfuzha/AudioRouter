use crate::com_service::device::get_output_device_by_id_internal;
use crate::router::{ChannelMode, RouterConfig};
use anyhow::{Result, anyhow};
use std::sync::Arc;
use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_SILENT, IAudioCaptureClient, IAudioClient, IAudioRenderClient, IMMDevice,
    WAVEFORMATEX,
};
use windows::Win32::System::Com::{CLSCTX_ALL, CoTaskMemFree};

#[derive(Clone)]
pub struct RouterSetupResult {
    pub _source_device: IMMDevice,
    pub source_client: IAudioClient,
    pub output_clients: Vec<RouterOutputClient>,
}

#[derive(Clone)]
pub struct RouterOutputClient {
    pub device_id: String,
    pub channel_mode: ChannelMode,
    pub client: IAudioClient,
}

#[derive(Clone)]
pub struct RouterInitialized {
    pub capture_service: IAudioCaptureClient,
    pub render_services: Vec<RouterRenderClient>,
}

#[derive(Clone)]
pub struct RouterRenderClient {
    pub channel_mode: ChannelMode,
    pub service: IAudioRenderClient,
}

pub struct MixFormat {
    ptr: *mut WAVEFORMATEX,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SampleFormat {
    F32,
    I16,
    I32,
    Unsupported,
}

impl MixFormat {
    fn new(ptr: *mut WAVEFORMATEX) -> Result<Self> {
        if ptr.is_null() {
            return Err(anyhow!("GetMixFormat returned null"));
        }
        Ok(Self { ptr })
    }

    fn as_ptr(&self) -> *const WAVEFORMATEX {
        self.ptr.cast_const()
    }
}

impl Drop for MixFormat {
    fn drop(&mut self) {
        unsafe {
            CoTaskMemFree(Some(self.ptr.cast()));
        }
    }
}

/// Internal function to create and initialize WASAPI audio clients for a router.
/// Must be called in a COM-initialized environment.
pub fn setup_router_clients(cfg: &RouterConfig) -> Result<RouterSetupResult> {
    let source_id = cfg
        .source_device_id
        .as_ref()
        .ok_or_else(|| anyhow!("source_device_id is required"))?;

    let source_device = get_output_device_by_id_internal(source_id)?;
    let source_client: IAudioClient = unsafe { source_device.Activate(CLSCTX_ALL, None) }
        .map_err(|e| anyhow!("Failed to activate source IAudioClient: {:?}", e))?;

    let mut output_clients = Vec::new();
    for target in &cfg.targets {
        match get_output_device_by_id_internal(&target.device_id) {
            Ok(dev) => match unsafe { dev.Activate::<IAudioClient>(CLSCTX_ALL, None) } {
                Ok(client) => output_clients.push(RouterOutputClient {
                    device_id: target.device_id.clone(),
                    channel_mode: target.channel_mode,
                    client,
                }),
                Err(e) => log::warn!(
                    "Failed to activate output device {}: {e:?}",
                    target.device_id
                ),
            },
            Err(e) => {
                log::warn!(
                    "Failed to resolve output device {}: {e:?}",
                    target.device_id
                );
            }
        }
    }

    if output_clients.is_empty() {
        return Err(anyhow!("No valid output devices found for routing"));
    }

    Ok(RouterSetupResult {
        _source_device: source_device,
        source_client,
        output_clients,
    })
}

pub fn get_mix_format(client: &IAudioClient) -> Result<MixFormat> {
    let pwf =
        unsafe { client.GetMixFormat() }.map_err(|e| anyhow!("GetMixFormat failed: {:?}", e))?;
    MixFormat::new(pwf)
}

/// Initialize a capture client for loopback. Must be called in COM thread.
fn initialize_capture_client_internal(
    client: &IAudioClient,
    pwf: *const WAVEFORMATEX,
) -> Result<IAudioCaptureClient> {
    use windows::Win32::Media::Audio::{AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK};

    let buffer_duration_100ns: i64 = 100_000_000; // 100ms
    unsafe {
        client
            .Initialize(
                windows::Win32::Media::Audio::AUDCLNT_SHAREMODE(AUDCLNT_SHAREMODE_SHARED.0),
                AUDCLNT_STREAMFLAGS_LOOPBACK,
                buffer_duration_100ns,
                0,
                pwf,
                None,
            )
            .map_err(|e| anyhow!("IAudioClient::Initialize (capture) failed: {:?}", e))?;

        client.GetService::<IAudioCaptureClient>().map_err(|e| {
            anyhow!(
                "IAudioClient::GetService (IAudioCaptureClient) failed: {:?}",
                e
            )
        })
    }
}

/// Initialize a render client. Must be called in COM thread.
fn initialize_render_client_internal(
    client: &IAudioClient,
    pwf: *const WAVEFORMATEX,
) -> Result<IAudioRenderClient> {
    use windows::Win32::Media::Audio::{
        AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
        AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
    };

    let buffer_duration_100ns: i64 = 100_000_000; // 100ms
    unsafe {
        client
            .Initialize(
                windows::Win32::Media::Audio::AUDCLNT_SHAREMODE(AUDCLNT_SHAREMODE_SHARED.0),
                AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                buffer_duration_100ns,
                0,
                pwf,
                None,
            )
            .map_err(|e| anyhow!("IAudioClient::Initialize (render) failed: {:?}", e))?;

        let service = client.GetService::<IAudioRenderClient>().map_err(|e| {
            anyhow!(
                "IAudioClient::GetService (IAudioRenderClient) failed: {:?}",
                e
            )
        })?;

        client
            .Start()
            .map_err(|e| anyhow!("IAudioClient::Start (render) failed: {:?}", e))?;

        Ok(service)
    }
}

/// High-level wrapper to initialize both capture and all renders.
pub fn initialize_router(
    capture: &IAudioClient,
    render_clients: &[RouterOutputClient],
    mix_format: &MixFormat,
) -> Result<RouterInitialized> {
    let pwf = mix_format.as_ptr();

    let capture_service = initialize_capture_client_internal(capture, pwf)?;

    let mut render_services = Vec::new();
    for render_client in render_clients {
        match initialize_render_client_internal(&render_client.client, pwf) {
            Ok(service) => {
                render_services.push(RouterRenderClient {
                    channel_mode: render_client.channel_mode,
                    service,
                });
            }
            Err(e) => log::warn!(
                "Failed to initialize render client {}: {e:?}",
                render_client.device_id
            ),
        }
    }

    if render_services.is_empty() {
        return Err(anyhow!("No render clients could be initialized"));
    }

    unsafe {
        capture
            .Start()
            .map_err(|e| anyhow!("IAudioClient::Start (capture) failed: {:?}", e))?;
    }

    Ok(RouterInitialized {
        capture_service,
        render_services,
    })
}

/// Process a single audio packet. Must be called in COM environment.
pub fn process_next_packet<F>(
    state: &RouterInitialized,
    mix_format: &MixFormat,
    cb: Arc<F>,
) -> Result<bool>
where
    F: Fn(&[f32], u32, u16) + Send + Sync + 'static,
{
    let capture = &state.capture_service;
    let renders = &state.render_services;
    let pwf = mix_format.as_ptr();

    unsafe {
        let packet_size = match capture.GetNextPacketSize() {
            Ok(s) => s,
            Err(e) => {
                return Err(anyhow!("GetNextPacketSize failed: {:?}", e));
            }
        };

        if packet_size == 0 {
            return Ok(false);
        }

        let mut buf_ptr: *mut u8 = std::ptr::null_mut();
        let mut frames: u32 = 0;
        let mut flags: u32 = 0;

        if let Err(e) = capture.GetBuffer(&mut buf_ptr, &mut frames, &mut flags, None, None) {
            return Err(anyhow!("GetBuffer failed: {:?}", e));
        }

        struct CaptureBufferGuard<'a> {
            capture: &'a IAudioCaptureClient,
            frames: u32,
        }

        impl Drop for CaptureBufferGuard<'_> {
            fn drop(&mut self) {
                unsafe {
                    let _ = self.capture.ReleaseBuffer(self.frames);
                }
            }
        }

        let _release_capture = CaptureBufferGuard { capture, frames };

        if frames > 0 && !buf_ptr.is_null() {
            let block_align = (*pwf).nBlockAlign as usize;
            let bytes = frames as usize * block_align;
            let slice = std::slice::from_raw_parts(buf_ptr as *const u8, bytes);

            let channels_count = (*pwf).nChannels as usize;
            let mut out_f32 = Vec::with_capacity(frames as usize * channels_count);

            let w_format = (*pwf).wFormatTag;
            let sample_format = detect_sample_format(pwf);
            let mut handled = false;

            let silent = (flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0;

            if silent {
                out_f32.resize(frames as usize * channels_count, 0.0);
                handled = true;
            } else if sample_format == SampleFormat::F32 {
                let samples = bytes / 4;
                let f32_slice: &[f32] =
                    std::slice::from_raw_parts(slice.as_ptr() as *const f32, samples);
                out_f32.extend_from_slice(f32_slice);
                handled = true;
            } else if sample_format == SampleFormat::I16 {
                let samples = bytes / 2;
                for i in 0..samples {
                    let b1 = slice[i * 2];
                    let b2 = slice[i * 2 + 1];
                    let val = i16::from_le_bytes([b1, b2]);
                    out_f32.push(val as f32 / 32768.0_f32);
                }
                handled = true;
            } else if sample_format == SampleFormat::I32 {
                let samples = bytes / 4;
                for i in 0..samples {
                    let b1 = slice[i * 4];
                    let b2 = slice[i * 4 + 1];
                    let b3 = slice[i * 4 + 2];
                    let b4 = slice[i * 4 + 3];
                    let val = i32::from_le_bytes([b1, b2, b3, b4]);
                    out_f32.push(val as f32 / 2147483648.0_f32);
                }
                handled = true;
            }

            if !handled {
                log::warn!("Unsupported audio format tag: {w_format}");
            }

            let sample_rate = (*pwf).nSamplesPerSec;
            let channels = channels_count as u16;

            if !out_f32.is_empty() {
                cb(&out_f32, sample_rate, channels);
            }

            for render in renders.iter() {
                match render.service.GetBuffer(frames) {
                    Ok(render_buf_ptr) => {
                        copy_with_channel_mode(
                            slice,
                            render_buf_ptr,
                            bytes,
                            channels_count,
                            sample_format,
                            render.channel_mode,
                            silent,
                        );
                        if let Err(_e) = render.service.ReleaseBuffer(frames, 0) {}
                    }
                    Err(e) => log::warn!("Failed to get render buffer: {e:?}"),
                }
            }

            Ok(true)
        } else {
            Ok(false)
        }
    }
}

fn detect_sample_format(pwf: *const WAVEFORMATEX) -> SampleFormat {
    const WAVE_FORMAT_PCM: u16 = 1;
    const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
    const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;

    unsafe {
        match ((*pwf).wFormatTag, (*pwf).wBitsPerSample) {
            (WAVE_FORMAT_IEEE_FLOAT, 32) => SampleFormat::F32,
            (WAVE_FORMAT_PCM, 16) => SampleFormat::I16,
            (WAVE_FORMAT_PCM, 32) => SampleFormat::I32,
            (WAVE_FORMAT_EXTENSIBLE, bits) => {
                let p_ext = pwf as *const windows::Win32::Media::Audio::WAVEFORMATEXTENSIBLE;
                let sub_format = (*p_ext).SubFormat;
                if sub_format.data1 == 0x00000003
                    && sub_format.data2 == 0x0000
                    && sub_format.data3 == 0x0010
                    && bits == 32
                {
                    SampleFormat::F32
                } else if sub_format.data1 == 0x00000001
                    && sub_format.data2 == 0x0000
                    && sub_format.data3 == 0x0010
                {
                    match bits {
                        16 => SampleFormat::I16,
                        32 => SampleFormat::I32,
                        _ => SampleFormat::Unsupported,
                    }
                } else {
                    SampleFormat::Unsupported
                }
            }
            _ => SampleFormat::Unsupported,
        }
    }
}

/// Cleanup and stop clients.
pub fn finalize_router(res: &RouterSetupResult) -> Result<()> {
    unsafe {
        let _ = res.source_client.Stop();
        for output in &res.output_clients {
            let _ = output.client.Stop();
        }
    }
    Ok(())
}

fn copy_with_channel_mode(
    source: &[u8],
    target: *mut u8,
    bytes: usize,
    channels: usize,
    sample_format: SampleFormat,
    mode: ChannelMode,
    silent: bool,
) {
    if silent {
        unsafe { std::ptr::write_bytes(target, 0, bytes) };
        return;
    }

    if channels != 2 || mode == ChannelMode::Stereo {
        unsafe { std::ptr::copy_nonoverlapping(source.as_ptr(), target, bytes) };
        return;
    }

    match sample_format {
        SampleFormat::F32 => copy_f32_stereo(source, target, mode),
        SampleFormat::I16 => copy_i16_stereo(source, target, mode),
        SampleFormat::I32 => copy_i32_stereo(source, target, mode),
        SampleFormat::Unsupported => {
            log::warn!(
                "Channel mode {:?} is unsupported for this format; using stereo",
                mode
            );
            unsafe { std::ptr::copy_nonoverlapping(source.as_ptr(), target, bytes) };
        }
    }
}

fn map_stereo_frame<T>(left: T, right: T, zero: T, mode: ChannelMode) -> (T, T)
where
    T: Copy + Average,
{
    match mode {
        ChannelMode::Stereo => (left, right),
        ChannelMode::LeftMono => (left, left),
        ChannelMode::RightMono => (right, right),
        ChannelMode::Mono => {
            let mixed = T::average(left, right);
            (mixed, mixed)
        }
        ChannelMode::Swap => (right, left),
        ChannelMode::LeftOnly => (left, zero),
        ChannelMode::RightOnly => (zero, right),
    }
}

trait Average {
    fn average(left: Self, right: Self) -> Self;
}

impl Average for f32 {
    fn average(left: Self, right: Self) -> Self {
        (left + right) * 0.5
    }
}

impl Average for i16 {
    fn average(left: Self, right: Self) -> Self {
        ((left as i32 + right as i32) / 2) as i16
    }
}

impl Average for i32 {
    fn average(left: Self, right: Self) -> Self {
        ((left as i64 + right as i64) / 2) as i32
    }
}

fn copy_f32_stereo(source: &[u8], target: *mut u8, mode: ChannelMode) {
    let samples = source.len() / 4;
    let input = unsafe { std::slice::from_raw_parts(source.as_ptr() as *const f32, samples) };
    let output = unsafe { std::slice::from_raw_parts_mut(target as *mut f32, samples) };
    apply_stereo_frames(input, output, 0.0, mode);
}

fn copy_i16_stereo(source: &[u8], target: *mut u8, mode: ChannelMode) {
    let samples = source.len() / 2;
    let input = unsafe { std::slice::from_raw_parts(source.as_ptr() as *const i16, samples) };
    let output = unsafe { std::slice::from_raw_parts_mut(target as *mut i16, samples) };
    apply_stereo_frames(input, output, 0, mode);
}

fn copy_i32_stereo(source: &[u8], target: *mut u8, mode: ChannelMode) {
    let samples = source.len() / 4;
    let input = unsafe { std::slice::from_raw_parts(source.as_ptr() as *const i32, samples) };
    let output = unsafe { std::slice::from_raw_parts_mut(target as *mut i32, samples) };
    apply_stereo_frames(input, output, 0, mode);
}

fn apply_stereo_frames<T>(input: &[T], output: &mut [T], zero: T, mode: ChannelMode)
where
    T: Copy + Average,
{
    for (src, dst) in input.chunks_exact(2).zip(output.chunks_exact_mut(2)) {
        let (left, right) = map_stereo_frame(src[0], src[1], zero, mode);
        dst[0] = left;
        dst[1] = right;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_f32_stereo_modes() {
        let input = [0.8_f32, 0.2_f32, -0.4_f32, 0.6_f32];
        let cases = [
            (ChannelMode::Stereo, vec![0.8, 0.2, -0.4, 0.6]),
            (ChannelMode::LeftMono, vec![0.8, 0.8, -0.4, -0.4]),
            (ChannelMode::RightMono, vec![0.2, 0.2, 0.6, 0.6]),
            (ChannelMode::Mono, vec![0.5, 0.5, 0.1, 0.1]),
            (ChannelMode::Swap, vec![0.2, 0.8, 0.6, -0.4]),
            (ChannelMode::LeftOnly, vec![0.8, 0.0, -0.4, 0.0]),
            (ChannelMode::RightOnly, vec![0.0, 0.2, 0.0, 0.6]),
        ];

        for (mode, expected) in cases {
            let mut output = vec![0.0_f32; input.len()];
            apply_stereo_frames(&input, &mut output, 0.0, mode);
            for (actual, expected) in output.iter().zip(expected) {
                assert!((actual - expected).abs() < f32::EPSILON);
            }
        }
    }
}
