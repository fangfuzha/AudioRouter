use crate::com_service::device::get_output_device_by_id_internal;
use crate::router::{ChannelMode, RouterConfig};
use anyhow::{Result, anyhow};
use std::sync::Arc;
use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_SILENT, IAudioCaptureClient, IAudioClient, IAudioRenderClient, IMMDevice,
    WAVEFORMATEX,
};
use windows::Win32::System::Com::{CLSCTX_ALL, CoTaskMemFree};

/// 设备 invalidated 相关的 HRESULT 代码。
/// 这些错误都表示设备状态发生变化（格式改变、设备移除/禁用等），
/// 需要重新初始化 WASAPI 客户端才能恢复路由。
const DEVICE_INVALIDATED_CODES: &[i32] = &[
    0x88870100u32 as i32, // AUDCLNT_E_DEVICE_INVALIDATED
    0x88890004u32 as i32, // AUDCLNT_E_NOT_STOPPED（格式改变时可能出现）
    0x88870101u32 as i32, // AUDCLNT_E_ALREADY_INITIALIZED
    0x8007001Fu32 as i32, // E_NOT_ACTIVATED (设备未激活)
    0x80004005u32 as i32, // E_FAIL (通用失败，某些驱动格式改变时返回)
];

/// 将 windows::core::Error 转换为不含 message() 的字符串，
/// 避免 windows 0.48.0 中 HRESULT::message() 在某些错误下
/// 触发 slice::from_raw_parts 的 UB precondition 检查而 panic。
fn err_code(e: &windows::core::Error) -> String {
    let code = e.code();
    let code_u32 = code.0 as u32;
    format!("0x{:08X}", code_u32)
}

/// 检查错误是否为设备 invalidated 或格式改变相关的可恢复错误。
/// 返回 true 时 worker 应尝试重启而非退出。
fn is_device_invalidated(e: &windows::core::Error) -> bool {
    let code = e.code().0;
    DEVICE_INVALIDATED_CODES.contains(&code)
}

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
    pub client: IAudioClient,
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
        .map_err(|e| anyhow!("Failed to activate source IAudioClient: {}", err_code(&e)))?;

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
                    "Failed to activate output device {}: {}",
                    target.device_id,
                    err_code(&e)
                ),
            },
            Err(e) => {
                log::warn!(
                    "Failed to resolve output device {}: {e}",
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
        unsafe { client.GetMixFormat() }.map_err(|e| anyhow!("GetMixFormat failed: {}", err_code(&e)))?;
    MixFormat::new(pwf)
}

/// Initialize a capture client for loopback. Must be called in COM thread.
fn initialize_capture_client_internal(
    client: &IAudioClient,
    pwf: *const WAVEFORMATEX,
) -> Result<IAudioCaptureClient> {
    use windows::Win32::Media::Audio::{AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK};

    let buffer_duration_100ns: i64 = 50_000_000; // 50ms
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
            .map_err(|e| anyhow!("IAudioClient::Initialize (capture) failed: {}", err_code(&e)))?;

        client.GetService::<IAudioCaptureClient>().map_err(|e| {
            anyhow!(
                "IAudioClient::GetService (IAudioCaptureClient) failed: {}",
                err_code(&e)
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

    let buffer_duration_100ns: i64 = 50_000_000; // 50ms
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
            .map_err(|e| anyhow!("IAudioClient::Initialize (render) failed: {}", err_code(&e)))?;

        let service = client.GetService::<IAudioRenderClient>().map_err(|e| {
            anyhow!(
                "IAudioClient::GetService (IAudioRenderClient) failed: {}",
                err_code(&e)
            )
        })?;

        client
            .Start()
            .map_err(|e| anyhow!("IAudioClient::Start (render) failed: {}", err_code(&e)))?;

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
                    client: render_client.client.clone(),
                    service,
                });
            }
            Err(e) => log::warn!(
                "Failed to initialize render client {}: {e}",
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
            .map_err(|e| anyhow!("IAudioClient::Start (capture) failed: {}", err_code(&e)))?;
    }

    Ok(RouterInitialized {
        capture_service,
        render_services,
    })
}

/// 目标缓冲延迟占总缓冲区大小的比例 (0.2 = 20%)。
/// 较低的目标延迟可以减少整体延迟，但太低会增加 underrun 风险。
const TARGET_BUFFER_RATIO: f64 = 0.2;

/// 判断是否应该跳过本次写入以降低累积延迟。
/// 当输出端 padding 超过目标阈值时，跳过整个 packet（而不是部分截断），
/// 这样可以避免波形断裂导致的噪点。
/// 返回 Ok(true) 表示跳过本次写入，Ok(false) 表示正常写入。
/// 返回 Err 表示设备 invalidated，调用方应传播错误触发重启。
fn should_skip_write(render_client: &IAudioClient) -> Result<bool> {
    unsafe {
        let padding = match render_client.GetCurrentPadding() {
            Ok(p) => p,
            Err(e) => {
                if is_device_invalidated(&e) {
                    return Err(anyhow!(
                        "Render device invalidated during GetCurrentPadding: {}",
                        err_code(&e)
                    ));
                }
                return Ok(false);
            }
        };

        let buffer_size = match render_client.GetBufferSize() {
            Ok(s) => s,
            Err(e) => {
                if is_device_invalidated(&e) {
                    return Err(anyhow!(
                        "Render device invalidated during GetBufferSize: {}",
                        err_code(&e)
                    ));
                }
                return Ok(false);
            }
        };

        if buffer_size == 0 {
            return Ok(false);
        }

        let target_padding = (buffer_size as f64 * TARGET_BUFFER_RATIO) as u32;
        Ok(padding > target_padding)
    }
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
                if is_device_invalidated(&e) {
                    return Err(anyhow!(
                        "Capture device invalidated (format changed or device removed): {}",
                        err_code(&e)
                    ));
                }
                return Err(anyhow!("GetNextPacketSize failed: {}", err_code(&e)));
            }
        };

        if packet_size == 0 {
            return Ok(false);
        }

        let mut buf_ptr: *mut u8 = std::ptr::null_mut();
        let mut frames: u32 = 0;
        let mut flags: u32 = 0;

        if let Err(e) = capture.GetBuffer(&mut buf_ptr, &mut frames, &mut flags, None, None) {
            if is_device_invalidated(&e) {
                return Err(anyhow!(
                    "Capture device invalidated during GetBuffer: {}",
                    err_code(&e)
                ));
            }
            return Err(anyhow!("GetBuffer failed: {}", err_code(&e)));
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
            let sample_rate = (*pwf).nSamplesPerSec;

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

            let channels = channels_count as u16;

            if !out_f32.is_empty() {
                cb(&out_f32, sample_rate, channels);
            }

            for render in renders.iter() {
                // 检查输出端累积延迟，padding 过高时跳过整个 packet，
                // 让输出端消化已缓冲数据。整个 packet 跳过可以避免
                // 部分截断导致的波形断裂和噪点。
                // should_skip_write 返回 Err 表示设备 invalidated，需传播错误触发重启。
                if should_skip_write(&render.client)? {
                    continue;
                }

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
                        if let Err(e) = render.service.ReleaseBuffer(frames, 0) {
                            if is_device_invalidated(&e) {
                                return Err(anyhow!(
                                    "Render device invalidated during ReleaseBuffer: {}",
                                    err_code(&e)
                                ));
                            }
                            log::warn!("ReleaseBuffer failed: {}", err_code(&e));
                        }
                    }
                    Err(e) => {
                        if is_device_invalidated(&e) {
                            return Err(anyhow!(
                                "Render device invalidated during GetBuffer: {}",
                                err_code(&e)
                            ));
                        }
                        log::warn!("Failed to get render buffer: {}", err_code(&e));
                    }
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
