//! Audio device enumeration and management.
//!
//! This module provides functionality to enumerate, query, and manage audio output devices
//! using Windows Core Audio APIs. It handles device discovery, state checking, and format
//! information retrieval in a thread-safe manner via the COM worker.

use crate::utils::{map_state, win_helpers};
use anyhow::{Result, anyhow};
use callcomapi_macros::with_com;
use serde::{Deserialize, Serialize};
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use windows::Win32::Media::Audio::{
    DEVICE_STATE_ACTIVE, IAudioClient, IMMDevice, IMMDeviceCollection, IMMDeviceEnumerator,
    MMDeviceEnumerator, eConsole, eRender,
};
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance, STGM_READ};

/// Device connection/state
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeviceState {
    Active,     // device is active and usable
    Disabled,   // device is disabled
    Unplugged,  // device is unplugged
    NotPresent, // device is not present
    Unknown,
}

/// Basic device info used by the rest of the system
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub id: String,            // Device ID
    pub friendly_name: String, // Friendly name
    pub state: DeviceState,    // Current device state
    pub channels: Option<u16>, // Number of channels
    /// Optional channel mask (WAVEFORMATEXTENSIBLE.dwChannelMask)
    pub channel_mask: Option<u32>, // Bitmask of speaker positions
    pub is_default: bool,      // Is this the default output device?
}

/// Internal function to get all output devices. Must be called in a COM-initialized environment.
///
/// This function enumerates all active audio rendering endpoints and collects
/// their information, including whether each is the default device.
///
/// # Returns
/// A vector of `DeviceInfo` for all active output devices.
///
/// # Errors
/// Returns an error if COM operations fail.
fn get_all_output_devices_internal() -> Result<Vec<DeviceInfo>> {
    let enumerator: IMMDeviceEnumerator =
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
            .map_err(|e| anyhow!("CoCreateInstance MMDeviceEnumerator failed: {:?}", e))?;

    let collection: IMMDeviceCollection =
        unsafe { enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) }
            .map_err(|e| anyhow!("EnumAudioEndpoints failed: {:?}", e))?;

    let count =
        unsafe { collection.GetCount() }.map_err(|e| anyhow!("GetCount failed: {:?}", e))? as u32;

    // Determine default device id so we can mark `is_default` correctly
    let default_device_id = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }
        .ok()
        .and_then(|dev| unsafe { dev.GetId() }.ok())
        .and_then(|id_pwstr| unsafe { id_pwstr.to_string() }.ok());

    let mut out = Vec::new();
    for i in 0..count {
        let device =
            unsafe { collection.Item(i) }.map_err(|e| anyhow!("Item({}) failed: {:?}", i, e))?;
        let info = get_device_info_internal(&device, default_device_id.as_deref())?;
        out.push(info);
    }

    Ok(out)
}

/// Internal function to get the default output device. Must be called in a COM-initialized environment.
///
/// # Returns
/// A `DeviceInfo` for the default audio output device.
///
/// # Errors
/// Returns an error if the default device cannot be retrieved or queried.
fn get_default_output_device_internal() -> Result<DeviceInfo> {
    let enumerator: IMMDeviceEnumerator =
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
            .map_err(|e| anyhow!("CoCreateInstance MMDeviceEnumerator failed: {:?}", e))?;

    let dev = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }
        .map_err(|e| anyhow!("GetDefaultAudioEndpoint failed: {:?}", e))?;
    let id_pwstr = unsafe { dev.GetId() }.map_err(|e| anyhow!("GetId failed: {:?}", e))?;
    let default_id = unsafe { id_pwstr.to_string() }.unwrap_or_default();

    get_device_info_internal(&dev, Some(&default_id))
}

/// Internal function to get a device by its ID. Must be called in a COM-initialized environment.
///
/// # Parameters
/// - `id`: The device ID string.
///
/// # Returns
/// The `IMMDevice` interface for the specified device.
///
/// # Errors
/// Returns an error if the device is not found or COM operations fail.
pub(super) fn get_output_device_by_id_internal(id: &str) -> Result<IMMDevice> {
    let enumerator: IMMDeviceEnumerator =
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
            .map_err(|e| anyhow!("CoCreateInstance MMDeviceEnumerator failed: {:?}", e))?;

    let wide: Vec<u16> = OsStr::new(id)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let pwstr = windows::core::PCWSTR(wide.as_ptr());

    unsafe { enumerator.GetDevice(pwstr) }.map_err(|e| anyhow!("GetDevice failed: {:?}", e))
}

/// Internal function to retrieve detailed information about a specific audio device.
///
/// This function queries the device's properties, state, and audio format information.
///
/// # Parameters
/// - `device`: Reference to the `IMMDevice` interface.
/// - `default_device_id`: Optional ID of the default device for comparison.
///
/// # Returns
/// A `DeviceInfo` struct with the device's details.
///
/// # Errors
/// Returns an error if property queries or format retrieval fails.
fn get_device_info_internal(
    device: &IMMDevice,
    default_device_id: Option<&str>,
) -> Result<DeviceInfo> {
    let id_pwstr = unsafe { device.GetId() }.map_err(|e| anyhow!("GetId failed: {:?}", e))?;
    let id = unsafe { id_pwstr.to_string() }.unwrap_or_else(|_| String::new());
    let state = unsafe { device.GetState().unwrap_or(0) };

    let mut friendly_name = id.clone();
    if let Ok(store) = unsafe { device.OpenPropertyStore(STGM_READ) } {
        if let Some(s) =
            unsafe { win_helpers::read_property_string(&store, &win_helpers::PKEY_DEVICE_FRIENDLY) }
        {
            friendly_name = s;
        }
    }

    let mut channels = None;
    let mut channel_mask = None;
    if let Ok(audio_client) = unsafe { device.Activate::<IAudioClient>(CLSCTX_ALL, None) } {
        if let Ok(pwf) = unsafe { audio_client.GetMixFormat() } {
            let (ch, mask) = crate::utils::parse_mix_format(pwf);
            channels = ch;
            channel_mask = mask;
        }
    }

    // Determine if this is the default device by comparing IDs. Note that `default_device_id` may be None if we failed to get it, in which case we'll just mark all devices as non-default.
    let is_default = default_device_id.map_or(false, |d| d == id);

    Ok(DeviceInfo {
        id,
        friendly_name,
        state: map_state(state),
        channels,
        channel_mask,
        is_default,
    })
}

/// Retrieves a list of all active audio output devices on the system.
/// This function is thread-safe and handles COM initialization internally via `#[with_com]`.
///
/// # Returns
/// A vector of `DeviceInfo` structs containing details about each device.
///
/// # Errors
/// Returns an error if device enumeration fails or COM operations encounter issues.
#[with_com]
pub fn get_all_output_devices() -> Result<Vec<DeviceInfo>> {
    get_all_output_devices_internal()
}

/// Retrieves information about the default audio output device.
///
/// # Returns
/// A `DeviceInfo` struct for the default device.
///
/// # Errors
/// Returns an error if the default device cannot be retrieved.
#[with_com]
pub fn get_default_output_device() -> Result<DeviceInfo> {
    get_default_output_device_internal()
}

/// Retrieves an audio device by its ID.
///
/// This function returns a `ComSend<IMMDevice>` to ensure the device interface
/// can be safely moved between threads.
///
/// # Parameters
/// - `id`: The device ID string.
///
/// # Returns
/// A `ComSend` wrapper containing the `IMMDevice` interface.
///
/// # Errors
/// Returns an error if the device with the given ID is not found.
#[with_com]
pub fn get_output_device_by_id(id: &str) -> Result<crate::utils::ComSend<IMMDevice>> {
    let id_str = id.to_string();
    get_output_device_by_id_internal(&id_str).map(crate::utils::ComSend::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "windows")]
    #[test]
    fn device_api_behaves() {
        // Windows-only integration test: enumerate devices and print details for inspection.
        let devices = get_all_output_devices().expect("list devices");
        if devices.is_empty() {
            println!("No audio output devices found.");
            return;
        } else {
            println!("Found {} audio output devices:", devices.len());
            for d in devices.iter() {
                println!(
                    " - id: {}, name: {}, state: {:?}, channels: {:?}, channel_mask: {:?}, is_default: {}",
                    d.id, d.friendly_name, d.state, d.channels, d.channel_mask, d.is_default
                );
            }
        }

        let d = get_default_output_device().expect("get_default");
        println!(
            "Default device: id: {}, name: {}, state: {:?}, channels: {:?}, channel_mask: {:?}, is_default: {}",
            d.id, d.friendly_name, d.state, d.channels, d.channel_mask, d.is_default
        );

        // Verify lookup by id for the first device
        let first_id = devices[0].id.clone();
        let found_dev = get_output_device_by_id(&first_id)
            .expect("lookup by id")
            .take();
        let id_pwstr = unsafe { found_dev.GetId() }.expect("GetId");
        let id_str = unsafe { id_pwstr.to_string() }.unwrap_or_default();
        assert_eq!(id_str, first_id);
    }
}
