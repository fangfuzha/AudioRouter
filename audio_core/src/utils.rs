use crate::com_service::device::DeviceState;

/// A wrapper to allow passing COM pointers/interfaces between threads safely.
#[derive(Debug, Clone)]
pub struct ComSend<T>(T);

unsafe impl<T> Send for ComSend<T> {}
unsafe impl<T> Sync for ComSend<T> {}

impl<T> ComSend<T> {
    pub fn new(t: T) -> Self {
        Self(t)
    }

    /// Consume the wrapper and return the underlying value.
    pub fn take(self) -> T {
        self.0
    }
}

impl<T: Send> ComSend<T> {
    pub fn unwrap(self) -> T {
        self.0
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn map_state(state: u32) -> DeviceState {
    use windows::Win32::Media::Audio::{
        DEVICE_STATE_ACTIVE, DEVICE_STATE_DISABLED, DEVICE_STATE_NOTPRESENT, DEVICE_STATE_UNPLUGGED,
    };
    if (state & DEVICE_STATE_ACTIVE) != 0 {
        DeviceState::Active
    } else if (state & DEVICE_STATE_DISABLED) != 0 {
        DeviceState::Disabled
    } else if (state & DEVICE_STATE_UNPLUGGED) != 0 {
        DeviceState::Unplugged
    } else if (state & DEVICE_STATE_NOTPRESENT) != 0 {
        DeviceState::NotPresent
    } else {
        DeviceState::Unknown
    }
}

/// Windows-only helpers for reading device properties
#[cfg(target_os = "windows")]
pub mod win_helpers {
    use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
    use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, PROPERTYKEY};
    use windows::core::GUID;

    #[link(name = "ole32")]
    unsafe extern "system" {
        pub fn PropVariantClear(pvar: *mut PROPVARIANT) -> i32;
        pub fn CoTaskMemFree(ppv: *mut core::ffi::c_void);
    }

    pub unsafe fn read_property_string(
        store: &IPropertyStore,
        key: &PROPERTYKEY,
    ) -> Option<String> {
        if let Ok(mut pv) = unsafe { store.GetValue(key) } {
            let mut result = None;
            unsafe {
                let p = pv.Anonymous.Anonymous.Anonymous.pwszVal;
                let raw = p.0 as *const u16;
                if !raw.is_null() {
                    let mut len = 0usize;
                    while *raw.add(len) != 0 {
                        len += 1;
                    }
                    let slice = std::slice::from_raw_parts(raw, len);
                    if let Ok(s) = String::from_utf16(slice) {
                        if !s.is_empty() {
                            result = Some(s);
                        }
                    }
                }
            }
            let _ = unsafe { PropVariantClear(&mut pv) };
            return result;
        }
        None
    }

    pub const PKEY_DEVICE_FRIENDLY: PROPERTYKEY = PROPERTYKEY {
        fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
        pid: 14,
    };
}

/// Decode a WAVEFORMATEXTENSIBLE `dwChannelMask` into readable speaker positions.
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

/// Parse a WAVEFORMATEX pointer returned by `IAudioClient::GetMixFormat`.
/// Returns (channels, optional channel_mask). The function will free the pointer via CoTaskMemFree.
#[cfg(target_os = "windows")]
pub fn parse_mix_format(
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
        let w_format = (*pwf).wFormatTag;
        if w_format == WAVE_FORMAT_EXTENSIBLE {
            #[allow(non_snake_case)]
            #[repr(C)]
            struct WAVEFORMATEXTENSIBLE {
                wf: WAVEFORMATEX,
                wValidBitsPerSample: u16,
                dwChannelMask: u32,
                SubFormat: windows::core::GUID,
            }
            let ext = pwf as *const WAVEFORMATEXTENSIBLE;
            channel_mask = Some((*ext).dwChannelMask);
        }

        // free the memory allocated by GetMixFormat
        win_helpers::CoTaskMemFree(pwf as *mut _);
        (Some(channels), channel_mask)
    }
}
