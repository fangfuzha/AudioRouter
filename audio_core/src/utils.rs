use crate::com_service::device::DeviceState;
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::System::Com::StructuredStorage::{PropVariantClear, PropVariantToStringAlloc};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
use windows::core::GUID;

/// Maps Windows device state code to DeviceState enum.
pub(crate) fn map_state(state: u32) -> DeviceState {
    use windows::Win32::Media::Audio::{
        DEVICE_STATE_ACTIVE, DEVICE_STATE_DISABLED, DEVICE_STATE_NOTPRESENT, DEVICE_STATE_UNPLUGGED,
    };
    if (state & DEVICE_STATE_ACTIVE.0) != 0 {
        DeviceState::Active
    } else if (state & DEVICE_STATE_DISABLED.0) != 0 {
        DeviceState::Disabled
    } else if (state & DEVICE_STATE_UNPLUGGED.0) != 0 {
        DeviceState::Unplugged
    } else if (state & DEVICE_STATE_NOTPRESENT.0) != 0 {
        DeviceState::NotPresent
    } else {
        DeviceState::Unknown
    }
}

/// Windows COM helpers for reading device properties.
pub mod win_helpers {
    use super::*;

    #[link(name = "ole32")]
    unsafe extern "system" {
        pub fn CoTaskMemFree(ppv: *mut core::ffi::c_void);
    }

    /// Reads a device property string from the property store.
    ///
    /// # Safety
    ///
    /// `store` must be a valid COM property store used on a thread whose COM
    /// apartment is initialized. The property value is cleared before return.
    pub unsafe fn read_property_string(
        store: &IPropertyStore,
        key: &PROPERTYKEY,
    ) -> Option<String> {
        if let Ok(mut pv) = unsafe { store.GetValue(key) } {
            // 用高层 API PropVariantToStringAlloc 解析字符串 variant,
            // 避免依赖 PROPVARIANT 内部 union 字段(0.58 已不可见)
            let result = unsafe { PropVariantToStringAlloc(&pv) }
                .ok()
                .and_then(|pwstr| {
                    let s = unsafe { pwstr.to_string() }.ok().filter(|s| !s.is_empty());
                    // 释放 PropVariantToStringAlloc 分配的内存
                    unsafe { CoTaskMemFree(pwstr.0 as *mut _) };
                    s
                });
            unsafe {
                let _ = PropVariantClear(&mut pv);
            };
            return result;
        }
        None
    }

    /// Property key for device-friendly name.
    pub const PKEY_DEVICE_FRIENDLY: PROPERTYKEY = PROPERTYKEY {
        fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
        pid: 14,
    };
}

/// Decodes a WAVEFORMATEXTENSIBLE channel mask into readable speaker positions.
///
/// Returns an ordered list of speaker position names for each set bit.
pub fn decode_channel_mask(mask: u32) -> Vec<&'static str> {
    const SPEAKER_FRONT_LEFT: u32 = 0x1;
    const SPEAKER_FRONT_RIGHT: u32 = 0x2;
    const SPEAKER_FRONT_CENTER: u32 = 0x4;
    const SPEAKER_LOW_FREQUENCY: u32 = 0x8;
    const SPEAKER_BACK_LEFT: u32 = 0x10;
    const SPEAKER_BACK_RIGHT: u32 = 0x20;
    const SPEAKER_FRONT_LEFT_OF_CENTER: u32 = 0x40;
    const SPEAKER_FRONT_RIGHT_OF_CENTER: u32 = 0x80;
    const SPEAKER_BACK_CENTER: u32 = 0x100;
    const SPEAKER_SIDE_LEFT: u32 = 0x200;
    const SPEAKER_SIDE_RIGHT: u32 = 0x400;
    const SPEAKER_TOP_CENTER: u32 = 0x800;
    const SPEAKER_TOP_FRONT_LEFT: u32 = 0x1000;
    const SPEAKER_TOP_FRONT_RIGHT: u32 = 0x2000;
    const SPEAKER_TOP_BACK_LEFT: u32 = 0x4000;
    const SPEAKER_TOP_BACK_RIGHT: u32 = 0x8000;

    let mut positions = Vec::new();
    if (mask & SPEAKER_FRONT_LEFT) != 0 {
        positions.push("Front Left");
    }
    if (mask & SPEAKER_FRONT_RIGHT) != 0 {
        positions.push("Front Right");
    }
    if (mask & SPEAKER_FRONT_CENTER) != 0 {
        positions.push("Front Center");
    }
    if (mask & SPEAKER_LOW_FREQUENCY) != 0 {
        positions.push("LFE");
    }
    if (mask & SPEAKER_BACK_LEFT) != 0 {
        positions.push("Back Left");
    }
    if (mask & SPEAKER_BACK_RIGHT) != 0 {
        positions.push("Back Right");
    }
    if (mask & SPEAKER_FRONT_LEFT_OF_CENTER) != 0 {
        positions.push("Front Left Of Center");
    }
    if (mask & SPEAKER_FRONT_RIGHT_OF_CENTER) != 0 {
        positions.push("Front Right Of Center");
    }
    if (mask & SPEAKER_BACK_CENTER) != 0 {
        positions.push("Back Center");
    }
    if (mask & SPEAKER_SIDE_LEFT) != 0 {
        positions.push("Side Left");
    }
    if (mask & SPEAKER_SIDE_RIGHT) != 0 {
        positions.push("Side Right");
    }
    if (mask & SPEAKER_TOP_CENTER) != 0 {
        positions.push("Top Center");
    }
    if (mask & SPEAKER_TOP_FRONT_LEFT) != 0 {
        positions.push("Top Front Left");
    }
    if (mask & SPEAKER_TOP_FRONT_RIGHT) != 0 {
        positions.push("Top Front Right");
    }
    if (mask & SPEAKER_TOP_BACK_LEFT) != 0 {
        positions.push("Top Back Left");
    }
    if (mask & SPEAKER_TOP_BACK_RIGHT) != 0 {
        positions.push("Top Back Right");
    }
    positions
}

/// Parses a WAVEFORMATEX pointer returned by `IAudioClient::GetMixFormat`.
///
/// Returns a tuple of `(channels, channel_mask)`. The pointer is freed via CoTaskMemFree.
///
/// # Safety
///
/// `pwf` must either be null or point to a valid `WAVEFORMATEX` buffer returned
/// by `IAudioClient::GetMixFormat`. This function always frees non-null input.
pub unsafe fn parse_mix_format(
    pwf: *const windows::Win32::Media::Audio::WAVEFORMATEX,
) -> (Option<u16>, Option<u32>) {
    use windows::Win32::Media::Audio::WAVEFORMATEX;

    if pwf.is_null() {
        return (None, None);
    }

    unsafe {
        let channels = (*pwf).nChannels;
        let mut channel_mask = None;

        const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;
        if (*pwf).wFormatTag == WAVE_FORMAT_EXTENSIBLE {
            #[allow(non_snake_case)]
            #[repr(C)]
            struct WaveFormatExtensible {
                wf: WAVEFORMATEX,
                wValidBitsPerSample: u16,
                dwChannelMask: u32,
                SubFormat: windows::core::GUID,
            }
            let ext = pwf as *const WaveFormatExtensible;
            channel_mask = Some((*ext).dwChannelMask);
        }

        // Free the memory allocated by GetMixFormat
        win_helpers::CoTaskMemFree(pwf as *mut _);
        (Some(channels), channel_mask)
    }
}

/// RAII guard for COM apartment initialization.
///
/// Calls `CoInitializeEx` on construction and `CoUninitialize` on drop,
/// ensuring COM is initialized for the duration of the scope.
pub(crate) struct ComApartment;

impl ComApartment {
    /// Initialize COM in multithreaded apartment (MTA) mode.
    pub(crate) fn mta() -> windows::core::Result<Self> {
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.ok()?;
        Ok(Self)
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}
