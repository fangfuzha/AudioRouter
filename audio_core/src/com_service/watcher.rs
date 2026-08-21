//! Audio device watcher COM operations.
//!
//! This module provides low-level COM-based functions for creating device enumerators
//! and managing notification clients for audio device changes. All operations are
//! performed through the COM environment to ensure thread safety and proper COM initialization.

use anyhow::{Result, anyhow};
use windows::Win32::Media::Audio::{
    IMMDeviceEnumerator, IMMNotificationClient, MMDeviceEnumerator,
};
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};

/// Internal: create the device enumerator. Must be called in COM.
pub(super) fn create_enumerator_internal() -> Result<IMMDeviceEnumerator> {
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
        .map_err(|e| anyhow!("CoCreateInstance MMDeviceEnumerator failed: {:?}", e))
}

/// Internal: register a notification client. Must be called in COM.
pub(super) fn register_notification_internal(
    enumerator: &IMMDeviceEnumerator,
    client: &IMMNotificationClient,
) -> Result<()> {
    unsafe {
        enumerator
            .RegisterEndpointNotificationCallback(client)
            .map_err(|e| anyhow!("RegisterEndpointNotificationCallback failed: {:?}", e))
    }
}

/// Internal: unregister a notification client. Must be called in COM.
pub(super) fn unregister_notification_internal(
    enumerator: &IMMDeviceEnumerator,
    client: &IMMNotificationClient,
) -> Result<()> {
    unsafe {
        enumerator
            .UnregisterEndpointNotificationCallback(client)
            .map_err(|e| anyhow!("UnregisterEndpointNotificationCallback failed: {:?}", e))
    }
}

/// Creates a new audio device enumerator instance.
/// Must be called in a COM-initialized environment.
///
/// # Errors
/// Returns an error if the COM object creation fails.
pub fn create_enumerator() -> Result<IMMDeviceEnumerator> {
    create_enumerator_internal()
}

/// Registers a notification client with the device enumerator.
/// Must be called in a COM-initialized environment.
///
/// # Errors
/// Returns an error if the registration fails.
pub fn register_notification(
    enumerator: &IMMDeviceEnumerator,
    client: &IMMNotificationClient,
) -> Result<()> {
    register_notification_internal(enumerator, client)
}

/// Unregisters a previously registered notification client from the device enumerator.
/// Must be called in a COM-initialized environment.
///
/// # Errors
/// Returns an error if the unregistration fails.
pub fn unregister_notification(
    enumerator: &IMMDeviceEnumerator,
    client: &IMMNotificationClient,
) -> Result<()> {
    unregister_notification_internal(enumerator, client)
}
