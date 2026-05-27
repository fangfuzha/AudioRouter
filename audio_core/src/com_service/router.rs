use crate::com_service::device::get_output_device_by_id_internal;
use crate::router::RouterConfig;
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
    /// (device_id, client)
    pub output_clients: Vec<(String, IAudioClient)>,
}

#[derive(Clone)]
pub struct RouterInitialized {
    pub capture_service: IAudioCaptureClient,
    pub render_services: Vec<IAudioRenderClient>,
}

pub struct MixFormat {
    ptr: *mut WAVEFORMATEX,
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
    for out_id in &cfg.target_device_ids {
        match get_output_device_by_id_internal(out_id) {
            Ok(dev) => match unsafe { dev.Activate::<IAudioClient>(CLSCTX_ALL, None) } {
                Ok(client) => output_clients.push((out_id.clone(), client)),
                Err(e) => log::warn!("Failed to activate output device {out_id}: {e:?}"),
            },
            Err(e) => {
                log::warn!("Failed to resolve output device {out_id}: {e:?}");
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
    render_clients: &[(String, IAudioClient)],
    mix_format: &MixFormat,
) -> Result<RouterInitialized> {
    let pwf = mix_format.as_ptr();

    let capture_service = initialize_capture_client_internal(capture, pwf)?;

    let mut render_services = Vec::new();
    for (device_id, render_client) in render_clients {
        match initialize_render_client_internal(&render_client, pwf) {
            Ok(service) => {
                render_services.push(service);
            }
            Err(e) => log::warn!("Failed to initialize render client {device_id}: {e:?}"),
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
            let mut handled = false;

            let silent = (flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0;

            if silent {
                out_f32.resize(frames as usize * channels_count, 0.0);
                handled = true;
            } else if w_format == 3u16 {
                let samples = bytes / 4;
                let f32_slice: &[f32] =
                    std::slice::from_raw_parts(slice.as_ptr() as *const f32, samples);
                out_f32.extend_from_slice(f32_slice);
                handled = true;
            } else if w_format == 1u16 {
                let samples = bytes / 2;
                for i in 0..samples {
                    let b1 = slice[i * 2];
                    let b2 = slice[i * 2 + 1];
                    let val = i16::from_le_bytes([b1, b2]);
                    out_f32.push(val as f32 / 32768.0_f32);
                }
                handled = true;
            } else if w_format == 0xFFFEu16 {
                // WAVE_FORMAT_EXTENSIBLE
                let p_ext = pwf as *const windows::Win32::Media::Audio::WAVEFORMATEXTENSIBLE;
                let sub_format = (*p_ext).SubFormat;

                // Check for KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
                // GUID: 00000003-0000-0010-8000-00aa00389b71
                if sub_format.data1 == 0x00000003
                    && sub_format.data2 == 0x0000
                    && sub_format.data3 == 0x0010
                {
                    let samples = bytes / 4;
                    let f32_slice: &[f32] =
                        std::slice::from_raw_parts(slice.as_ptr() as *const f32, samples);
                    out_f32.extend_from_slice(f32_slice);
                    handled = true;
                }
                // Check for KSDATAFORMAT_SUBTYPE_PCM
                // GUID: 00000001-0000-0010-8000-00aa00389b71
                else if sub_format.data1 == 0x00000001
                    && sub_format.data2 == 0x0000
                    && sub_format.data3 == 0x0010
                {
                    let bits = (*pwf).wBitsPerSample;
                    if bits == 16 {
                        let samples = bytes / 2;
                        for i in 0..samples {
                            let b1 = slice[i * 2];
                            let b2 = slice[i * 2 + 1];
                            let val = i16::from_le_bytes([b1, b2]);
                            out_f32.push(val as f32 / 32768.0_f32);
                        }
                        handled = true;
                    } else if bits == 32 {
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
                }
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
                match render.GetBuffer(frames) {
                    Ok(render_buf_ptr) => {
                        std::ptr::copy_nonoverlapping(buf_ptr, render_buf_ptr, bytes);
                        if let Err(_e) = render.ReleaseBuffer(frames, 0) {}
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

/// Cleanup and stop clients.
pub fn finalize_router(res: &RouterSetupResult) -> Result<()> {
    unsafe {
        let _ = res.source_client.Stop();
        for (_, client) in &res.output_clients {
            let _ = client.Stop();
        }
    }
    Ok(())
}
