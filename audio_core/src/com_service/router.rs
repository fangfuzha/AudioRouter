use crate::channel_mixer::ChannelMixer;
use crate::com_service::device::get_output_device_by_id_internal;
use crate::com_worker::ComSend;
use crate::router::RouterConfig;
use anyhow::{Result, anyhow};
use std::sync::Arc;
use windows::Win32::Media::Audio::{IAudioCaptureClient, WAVEFORMATEX};
use windows::Win32::Media::Audio::{IAudioClient, IAudioRenderClient, IMMDevice};
use windows::Win32::System::Com::CLSCTX_ALL;

#[derive(Clone)]
pub struct RouterSetupResult {
    pub source_device: IMMDevice,
    pub source_client: IAudioClient,
    /// (device_id, client)
    pub output_clients: Vec<(String, IAudioClient)>,
}

#[derive(Clone)]
pub struct RouterInitialized {
    pub capture_service: IAudioCaptureClient,
    pub render_services: Vec<IAudioRenderClient>,
}

/// Internal function to create and initialize WASAPI audio clients for a router.
/// Must be called in a COM-initialized environment.
fn setup_router_clients_internal(cfg: &RouterConfig) -> Result<RouterSetupResult> {
    let source_id = cfg
        .source_device_id
        .as_ref()
        .ok_or_else(|| anyhow!("source_device_id is required"))?;

    // Get source device and activated client
    let source_device = get_output_device_by_id_internal(source_id)?;
    let source_client: IAudioClient = unsafe { source_device.Activate(CLSCTX_ALL, None) }
        .map_err(|e| anyhow!("Failed to activate source IAudioClient: {:?}", e))?;

    let mut output_clients = Vec::new();
    for (out_id, _) in &cfg.target_config {
        if let Ok(dev) = get_output_device_by_id_internal(out_id) {
            if let Ok(client) = unsafe { dev.Activate::<IAudioClient>(CLSCTX_ALL, None) } {
                output_clients.push((out_id.clone(), client));
            }
        }
    }

    if output_clients.is_empty() {
        return Err(anyhow!("No valid output devices found for routing"));
    }

    Ok(RouterSetupResult {
        source_device,
        source_client,
        output_clients,
    })
}

fn get_mix_format_internal(client: &IAudioClient) -> Result<Vec<u8>> {
    let pwf =
        unsafe { client.GetMixFormat() }.map_err(|e| anyhow!("GetMixFormat failed: {:?}", e))?;
    if pwf.is_null() {
        return Err(anyhow!("GetMixFormat returned null"));
    }

    let size = std::mem::size_of::<WAVEFORMATEX>() + unsafe { (*pwf).cbSize } as usize;
    let slice = unsafe { std::slice::from_raw_parts(pwf as *const u8, size) };
    let vec = slice.to_vec();

    unsafe {
        windows::Win32::System::Com::CoTaskMemFree(Some(pwf as *mut _));
    }

    Ok(vec)
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
                windows::Win32::Media::Audio::AUDCLNT_SHAREMODE(AUDCLNT_SHAREMODE_SHARED.0 as i32),
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
                windows::Win32::Media::Audio::AUDCLNT_SHAREMODE(AUDCLNT_SHAREMODE_SHARED.0 as i32),
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

/// Get the mix format for an audio client via ComWorker
pub fn get_mix_format(client: ComSend<IAudioClient>) -> Result<ComSend<Vec<u8>>> {
    crate::com_worker::global().call_sync(move || get_mix_format_internal(&client.take()))
}

/// Helper that wraps client setup in ComWorker
pub fn setup_router_clients(cfg: RouterConfig) -> Result<ComSend<RouterSetupResult>> {
    crate::com_worker::global().call_sync(move || setup_router_clients_internal(&cfg))
}

/// High-level wrapper to initialize both capture and all renders.
pub fn initialize_router(
    capture_client: ComSend<IAudioClient>,
    render_clients: Vec<ComSend<IAudioClient>>,
    pwf_bytes: ComSend<Vec<u8>>,
) -> Result<ComSend<RouterInitialized>> {
    crate::com_worker::global().call_sync(move || {
        let capture = capture_client.take();
        let pwf_vec = pwf_bytes.take();
        let pwf = pwf_vec.as_ptr() as *const WAVEFORMATEX;

        // Initialize capture
        let capture_service = initialize_capture_client_internal(&capture, pwf)?;

        let mut render_services = Vec::new();
        for render_client in render_clients {
            let render_client = render_client.take();
            // Initialize render
            match initialize_render_client_internal(&render_client, pwf) {
                Ok(service) => {
                    render_services.push(service);
                }
                Err(e) => eprintln!("Failed to initialize render client with source format: {:?}. This is likely due to format mismatch (e.g. sample rate difference).", e),
            }
        }

        // Start capture
        unsafe {
            capture
                .Start()
                .map_err(|e| anyhow!("IAudioClient::Start (capture) failed: {:?}", e))?;
        }

        Ok(RouterInitialized {
            capture_service,
            render_services,
        })
    })
}

/// Process a single audio packet. Must be called in COM thread.
pub fn process_next_packet<F>(
    init_res: ComSend<RouterInitialized>,
    pwf_bytes: ComSend<Vec<u8>>,
    mixers: Vec<ChannelMixer>,
    cb: Arc<F>,
) -> Result<bool>
where
    F: Fn(&[f32], u32, u16) + Send + Sync + 'static,
{
    crate::com_worker::global()
        .call_sync(move || {
            let state = init_res.take();
            let capture = state.capture_service;
            let renders = state.render_services;
            let pwf_vec = pwf_bytes.take();
            let pwf = pwf_vec.as_ptr() as *const WAVEFORMATEX;

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

                if let Err(e) = capture.GetBuffer(&mut buf_ptr, &mut frames, &mut flags, None, None)
                {
                    return Err(anyhow!("GetBuffer failed: {:?}", e));
                }

                if frames > 0 && !buf_ptr.is_null() {
                    let block_align = (*pwf).nBlockAlign as usize;
                    let bytes = frames as usize * block_align;
                    let slice = std::slice::from_raw_parts(buf_ptr as *const u8, bytes);

                    // --- 1. Clone to callback ---
                    let channels_count = (*pwf).nChannels as usize;
                    let mut out_f32 = Vec::with_capacity(frames as usize * channels_count);

                    // Handle different formats
                    let w_format = (*pwf).wFormatTag as u16;
                    let mut handled = false;

                    if w_format == 3u16 {
                        // IEEE float32
                        let samples = (bytes / 4) as usize;
                        let f32_slice: &[f32] =
                            std::slice::from_raw_parts(slice.as_ptr() as *const f32, samples);
                        out_f32.extend_from_slice(f32_slice);
                        handled = true;
                    } else if w_format == 1u16 {
                        // PCM16
                        let samples = (bytes / 2) as usize;
                        for i in 0..samples {
                            let b1 = slice[i * 2];
                            let b2 = slice[i * 2 + 1];
                            let val = i16::from_le_bytes([b1, b2]);
                            out_f32.push(val as f32 / 32768.0_f32);
                        }
                        handled = true;
                    } else if w_format == 0xFFFEu16 {
                        // WAVE_FORMAT_EXTENSIBLE
                        let p_ext =
                            pwf as *const windows::Win32::Media::Audio::WAVEFORMATEXTENSIBLE;
                        let sub_format = (*p_ext).SubFormat;

                        // Check for KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
                        // GUID: 00000003-0000-0010-8000-00aa00389b71
                        if sub_format.data1 == 0x00000003
                            && sub_format.data2 == 0x0000
                            && sub_format.data3 == 0x0010
                        {
                            let samples = (bytes / 4) as usize;
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
                            let bits = (*pwf).wBitsPerSample as u16;
                            if bits == 16 {
                                let samples = (bytes / 2) as usize;
                                for i in 0..samples {
                                    let b1 = slice[i * 2];
                                    let b2 = slice[i * 2 + 1];
                                    let val = i16::from_le_bytes([b1, b2]);
                                    out_f32.push(val as f32 / 32768.0_f32);
                                }
                                handled = true;
                            } else if bits == 32 {
                                let samples = (bytes / 4) as usize;
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
                        // Fallback or warning
                    }

                    let sample_rate = (*pwf).nSamplesPerSec as u32;
                    let channels = channels_count as u16;

                    // Call callback if we have data
                    if !out_f32.is_empty() {
                        cb(&out_f32, sample_rate, channels);
                    }

                    // --- 2. Clone to all output devices ---
                    for (i, render) in renders.iter().enumerate() {
                        if i >= mixers.len() {
                            continue; // Should not happen
                        }
                        let mixer = &mixers[i];

                        match render.GetBuffer(frames) {
                            Ok(render_buf_ptr) => {
                                if handled && w_format == 3u16 {
                                    // IEEE float32 format: mix f32 samples and copy
                                    let mixed_samples =
                                        mixer.process_samples(&out_f32, channels_count);
                                    let mixed_bytes = std::slice::from_raw_parts(
                                        mixed_samples.as_ptr() as *const u8,
                                        mixed_samples.len() * std::mem::size_of::<f32>(),
                                    );
                                    std::ptr::copy_nonoverlapping(
                                        mixed_bytes.as_ptr(),
                                        render_buf_ptr,
                                        bytes,
                                    );
                                } else {
                                    // Fallback: copy original buffer
                                    std::ptr::copy_nonoverlapping(buf_ptr, render_buf_ptr, bytes);
                                }
                                if let Err(_e) = render.ReleaseBuffer(frames, 0) {
                                    // Silently ignore or log once?
                                }
                            }
                            Err(_e) => {
                                // If buffer is too large, it means we are out of sync or
                                // the output buffer is full. In a simple loopback,
                                // we might just skip this packet.
                            }
                        }
                    }

                    let _ = capture.ReleaseBuffer(frames);
                    Ok(true)
                } else {
                    let _ = capture.ReleaseBuffer(frames);
                    Ok(false)
                }
            }
        })
        .map(|s| s.unwrap())
}

/// Cleanup and stop clients.
pub fn finalize_router(setup_res: ComSend<RouterSetupResult>) -> Result<()> {
    crate::com_worker::global()
        .call_sync(move || {
            let res = setup_res.take();
            unsafe {
                let _ = res.source_client.Stop();
                for (_, client) in res.output_clients {
                    let _ = client.Stop();
                }
            }
            Ok(())
        })
        .map(|s| s.unwrap())
}
