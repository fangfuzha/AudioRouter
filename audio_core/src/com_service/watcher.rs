//! Audio device watcher COM operations.
//!
//! This module provides low-level COM-based functions for creating device enumerators
//! and managing notification clients for audio device changes. All operations are
//! performed through the COM worker to ensure thread safety and proper COM initialization.

use crate::com_worker::ComSend;
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

/// Helper that wraps enumerator creation in ComWorker
///
/// Creates a new audio device enumerator instance. This function is thread-safe
/// and ensures the enumerator is created in a properly initialized COM environment.
///
/// # Returns
/// A `ComSend` wrapper containing the `IMMDeviceEnumerator` interface.
///
/// # Errors
/// Returns an error if the COM object creation fails.
pub fn create_enumerator() -> Result<ComSend<IMMDeviceEnumerator>> {
    crate::com_worker::global().call_sync(|| create_enumerator_internal())
}

/// Register a notification client via ComWorker
///
/// Registers a notification client with the device enumerator to receive callbacks
/// for audio device changes (add, remove, state changes, etc.). The registration
/// is performed in a COM-initialized thread for safety.
///
/// # Parameters
/// - `enumerator`: A `ComSend` wrapper containing the device enumerator.
/// - `client`: A `ComSend` wrapper containing the notification client.
///
/// # Returns
/// A `ComSend` wrapper containing an empty tuple on success.
///
/// # Errors
/// Returns an error if the registration fails or COM operations encounter issues.
pub fn register_notification(
    enumerator: ComSend<IMMDeviceEnumerator>,
    client: ComSend<IMMNotificationClient>,
) -> Result<ComSend<()>> {
    crate::com_worker::global()
        .call_sync(move || register_notification_internal(&enumerator.take(), &client.take()))
}

/// Unregister a notification client via ComWorker
///
/// Unregisters a previously registered notification client from the device enumerator.
/// This stops receiving callbacks for device changes. The unregistration is performed
/// in a COM-initialized thread for safety.
///
/// # Parameters
/// - `enumerator`: A `ComSend` wrapper containing the device enumerator.
/// - `client`: A `ComSend` wrapper containing the notification client to unregister.
///
/// # Returns
/// A `ComSend` wrapper containing an empty tuple on success.
///
/// # Errors
/// Returns an error if the unregistration fails or COM operations encounter issues.
pub fn unregister_notification(
    enumerator: ComSend<IMMDeviceEnumerator>,
    client: ComSend<IMMNotificationClient>,
) -> Result<ComSend<()>> {
    crate::com_worker::global()
        .call_sync(move || unregister_notification_internal(&enumerator.take(), &client.take()))
}
