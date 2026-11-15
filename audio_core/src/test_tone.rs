//! 向指定输出设备播放一段短提示音。
//!
//! 用途：多台音箱/显示器时，用户可以点「试听」确认某个设备到底是哪一个，
//! 避免盲选。实现上直接开一个 WASAPI 共享模式渲染客户端，写入一段
//! 48kHz / 16bit 立体声正弦波；借助 `AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM`
//! 由音频引擎负责转换成设备实际支持的格式，因此不需要读取混音格式。

use std::time::Duration;

use anyhow::{Result, anyhow};
use windows::Win32::Media::Audio::{
    AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
    AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, IAudioClient, IAudioRenderClient, WAVEFORMATEX,
};
use windows::Win32::System::Com::CLSCTX_ALL;

use crate::com_service::device::get_output_device_by_id;
use crate::utils::ComApartment;

/// 我们固定生成的音频格式参数。
const SAMPLE_RATE: u32 = 48_000;
const CHANNELS: u16 = 2;
const BITS_PER_SAMPLE: u16 = 16;
/// 提示音频率。
const TONE_HZ: f64 = 880.0;
/// 提示音总时长。
const TONE_DURATION: Duration = Duration::from_millis(500);
/// 淡入淡出时长，避免起止爆音。
const FADE: Duration = Duration::from_millis(20);
/// 缓冲区时长（向上取整到引擎要求）。
const BUFFER_DURATION_100NS: i64 = 2_000_000; // 200ms 100ns units

/// 在 `device_id` 对应的输出设备上播放一段提示音（阻塞直到播放完毕）。
///
/// 调用方应在后台线程执行，避免阻塞 UI。
pub fn play(device_id: &str) -> Result<()> {
    let _com = ComApartment::mta()?;

    let device = get_output_device_by_id(device_id)?;
    // SAFETY: COM 已初始化（ComApartment），device 是有效的 IMMDevice。
    let client: IAudioClient = unsafe { device.Activate(CLSCTX_ALL, None) }
        .map_err(|e| anyhow!("Failed to activate device for test tone: {e}"))?;

    let format = wave_format();
    unsafe {
        client
            .Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                BUFFER_DURATION_100NS,
                0,
                &format,
                None,
            )
            .map_err(|e| anyhow!("Failed to initialize test tone client: {e}"))?;

        let render: IAudioRenderClient = client
            .GetService()
            .map_err(|e| anyhow!("Failed to get render service: {e}"))?;

        client
            .Start()
            .map_err(|e| anyhow!("Failed to start test tone client: {e}"))?;

        let result = write_tone(&client, &render);

        // 无论写入是否成功都要停止客户端，避免设备一直被占用。
        let _ = client.Stop();
        result
    }
}

/// 生成 PCM 格式描述（16bit 立体声 48kHz）。
fn wave_format() -> WAVEFORMATEX {
    let block_align = CHANNELS * BITS_PER_SAMPLE / 8;
    WAVEFORMATEX {
        // WAVE_FORMAT_PCM
        wFormatTag: 1,
        nChannels: CHANNELS,
        nSamplesPerSec: SAMPLE_RATE,
        nAvgBytesPerSec: SAMPLE_RATE * u32::from(block_align),
        nBlockAlign: block_align,
        wBitsPerSample: BITS_PER_SAMPLE,
        cbSize: 0,
    }
}

/// 循环写入正弦波数据直到累计时长达到 [`TONE_DURATION`]。
///
/// # Safety
///
/// `client` 必须是已 `Start()` 且用 16bit 立体声格式初始化的渲染客户端。
unsafe fn write_tone(client: &IAudioClient, render: &IAudioRenderClient) -> Result<()> {
    let total_frames = (u64::from(SAMPLE_RATE) * TONE_DURATION.as_millis() as u64 / 1000) as u32;
    let fade_frames = (u64::from(SAMPLE_RATE) * FADE.as_millis() as u64 / 1000) as u32;
    let mut written: u32 = 0;

    while written < total_frames {
        let padding = unsafe { client.GetCurrentPadding() }
            .map_err(|e| anyhow!("GetCurrentPadding failed: {e}"))?;
        let buffer_size =
            unsafe { client.GetBufferSize() }.map_err(|e| anyhow!("GetBufferSize failed: {e}"))?;
        let available = buffer_size.saturating_sub(padding);
        if available == 0 {
            // 缓冲区已满，等一会再写。
            std::thread::sleep(Duration::from_millis(5));
            continue;
        }

        let frames = available.min(total_frames - written);
        let data = unsafe { render.GetBuffer(frames) }
            .map_err(|e| anyhow!("IAudioRenderClient::GetBuffer failed: {e}"))?;

        let samples = unsafe {
            std::slice::from_raw_parts_mut(data as *mut i16, frames as usize * CHANNELS as usize)
        };
        fill_sine(samples, written, frames, total_frames, fade_frames);

        unsafe { render.ReleaseBuffer(frames, 0) }
            .map_err(|e| anyhow!("IAudioRenderClient::ReleaseBuffer failed: {e}"))?;
        written += frames;

        std::thread::sleep(Duration::from_millis(5));
    }

    // 等设备把最后一段缓冲播完，否则 Stop 会截断尾音。
    let mut drained = 0;
    while drained < 20 {
        let padding = unsafe { client.GetCurrentPadding() }.unwrap_or(0);
        if padding == 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
        drained += 1;
    }
    Ok(())
}

/// 把 `frames` 帧正弦波写入 `samples`（左右声道相同，带淡入淡出）。
fn fill_sine(samples: &mut [i16], start_frame: u32, frames: u32, total: u32, fade: u32) {
    for frame in 0..frames {
        let index = start_frame + frame;
        let t = f64::from(index) / f64::from(SAMPLE_RATE);
        let mut amplitude = 0.25; // -12 dBFS，避免音量过冲

        if fade > 0 {
            let from_start = index;
            let to_end = total.saturating_sub(index);
            let distance = from_start.min(to_end);
            if distance < fade {
                amplitude *= f64::from(distance) / f64::from(fade);
            }
        }

        let value = (amplitude * (2.0 * std::f64::consts::PI * TONE_HZ * t).sin() * 32767.0) as i16;
        let base = frame as usize * CHANNELS as usize;
        samples[base] = value;
        if CHANNELS > 1 {
            samples[base + 1] = value;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fade_ramps_amplitude_down_at_both_ends() {
        let total = 1000u32;
        let fade = 100u32;
        let mut samples = vec![0i16; total as usize * CHANNELS as usize];

        fill_sine(&mut samples, 0, total, total, fade);

        // 第一帧完全静音（淡入起点），中间有声音，末尾静音（淡出终点）。
        assert_eq!(samples[0], 0);
        let middle = total as usize / 2 * CHANNELS as usize;
        assert_ne!(samples[middle], 0);
        let last = (total as usize - 1) * CHANNELS as usize;
        assert!(samples[last].abs() < 100, "tail should be near silent");
    }

    #[test]
    fn wave_format_matches_16_bit_stereo() {
        // WAVEFORMATEX 是 packed 结构，字段不能取引用，先复制到本地变量。
        let f = wave_format();
        let tag = f.wFormatTag;
        let channels = f.nChannels;
        let block_align = f.nBlockAlign;
        let avg_bytes = f.nAvgBytesPerSec;
        assert_eq!(tag, 1, "WAVE_FORMAT_PCM");
        assert_eq!(channels, 2);
        assert_eq!(block_align, 4);
        assert_eq!(avg_bytes, SAMPLE_RATE * 4);
    }
}
