//! Audio device change watcher.
//!
//! This module provides functionality to monitor changes in audio devices,
//! such as device addition, removal, state changes, and default device changes.
//! It uses Windows COM APIs to register for notifications and forwards events
//! through a channel for easy consumption by other parts of the application.

use anyhow::Result;
use std::sync::mpsc::{self, RecvTimeoutError, Receiver, Sender};
use std::thread;
use std::time::Duration;
use windows::core::implement;

use crate::com_service::device::{DeviceInfo, get_default_output_device};

/// Event types for device changes.
///
/// # Example
///
/// ```no_run
/// use audio_core::device_watcher::{DeviceWatcher, DeviceEvent};
///
/// // Start watcher and receive events on the returned receiver
/// let (mut watcher, rx) = DeviceWatcher::start().expect("start watcher");
/// // Receive one event (initial default device will be sent on start)
/// if let Ok(evt) = rx.recv() {
///     match evt {
///         DeviceEvent::DefaultChanged(opt) => {
///             println!("Default device changed: {:?}", opt);
///         }
///         DeviceEvent::Changed => {
///             println!("Device topology changed; call list_output_devices() to refresh");
///         }
///     }
/// }
/// // Stop when done
/// watcher.stop();
/// ```
#[derive(Debug, Clone)]
pub enum DeviceEvent {
    /// Something changed in device topology (add/remove/state).
    Changed,
    /// Default device changed; contains current default device.
    DefaultChanged(DeviceInfo),
}

/// Notification client for Windows COM device events.
#[implement(windows::Win32::Media::Audio::IMMNotificationClient)]
pub struct NotificationClient {
    sender: Sender<DeviceEvent>,
}

impl NotificationClient {
    /// Creates a new notification client.
    pub fn new(sender: Sender<DeviceEvent>) -> Self {
        Self { sender }
    }
}

impl windows::Win32::Media::Audio::IMMNotificationClient_Impl for NotificationClient {
    fn OnDeviceStateChanged(
        &self,
        _pwstrdeviceid: &windows::core::PCWSTR,
        _dwnewstate: u32,
    ) -> windows::core::Result<()> {
        let _ = self.sender.send(DeviceEvent::Changed);
        Ok(())
    }

    fn OnDeviceAdded(&self, _pwstrdeviceid: &windows::core::PCWSTR) -> windows::core::Result<()> {
        let _ = self.sender.send(DeviceEvent::Changed);
        Ok(())
    }

    fn OnDeviceRemoved(&self, _pwstrdeviceid: &windows::core::PCWSTR) -> windows::core::Result<()> {
        let _ = self.sender.send(DeviceEvent::Changed);
        Ok(())
    }

    fn OnDefaultDeviceChanged(
        &self,
        _flow: windows::Win32::Media::Audio::EDataFlow,
        _role: windows::Win32::Media::Audio::ERole,
        _pwstrdefaultdeviceid: &windows::core::PCWSTR,
    ) -> windows::core::Result<()> {
        match get_default_output_device() {
            Ok(d) => {
                let _ = self.sender.send(DeviceEvent::DefaultChanged(d));
            }
            Err(e) => log::error!("get_default_output_device failed in callback: {:?}", e),
        }
        Ok(())
    }

    fn OnPropertyValueChanged(
        &self,
        _pwstrdeviceid: &windows::core::PCWSTR,
        _key: &windows::Win32::UI::Shell::PropertiesSystem::PROPERTYKEY,
    ) -> windows::core::Result<()> {
        let _ = self.sender.send(DeviceEvent::Changed);
        Ok(())
    }
}

/// Handle for the device watcher.
///
/// Drop or call `stop()` to unregister and stop the background thread.
pub struct DeviceWatcher {
    stop_tx: Option<Sender<()>>,
    join_handle: Option<thread::JoinHandle<()>>,
}

impl DeviceWatcher {
    /// Starts a device watcher and returns the handle and event receiver.
    ///
    /// This spawns a background thread that registers for audio device notifications.
    /// An initial `DefaultChanged` event is sent immediately with the current default device.
    ///
    /// # Returns
    /// A tuple of `(DeviceWatcher, Receiver<DeviceEvent>)`.
    ///
    /// # Errors
    /// Returns an error if COM setup fails.
    pub fn start() -> Result<(DeviceWatcher, Receiver<DeviceEvent>)> {
        let (event_tx, event_rx) = mpsc::channel::<DeviceEvent>();
        let (stop_tx, stop_rx) = mpsc::channel::<()>();

        let join_handle = thread::spawn(move || {
            if let Err(e) = watcher_thread(event_tx, stop_rx) {
                log::error!("Watcher thread error: {:?}", e);
            }
        });

        Ok((
            DeviceWatcher {
                stop_tx: Some(stop_tx),
                join_handle: Some(join_handle),
            },
            event_rx,
        ))
    }

    /// Stops the watcher and waits for the background thread to exit.
    ///
    /// This method is idempotent and safe to call multiple times.
    pub fn stop(&mut self) {
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(());
        }
        if let Some(h) = self.join_handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for DeviceWatcher {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Main watcher thread function.
fn watcher_thread(
    event_tx: Sender<DeviceEvent>,
    stop_rx: Receiver<()>,
) -> Result<()> {
    let enumerator = crate::com_service::watcher::create_enumerator()?;

    // Create the COM notification client
    let client: windows::Win32::Media::Audio::IMMNotificationClient =
        NotificationClient::new(event_tx.clone()).into();

    // Register for notifications
    crate::com_service::watcher::register_notification(
        enumerator.clone(),
        crate::utils::ComSend::new(client.clone()),
    )?;

    // Send initial default device event
    if let Ok(d) = get_default_output_device() {
        event_tx.send(DeviceEvent::DefaultChanged(d))?;
    }

    // Wait for stop signal
    watcher_event_loop(&stop_rx)?;

    // Unregister callback
    let _ = crate::com_service::watcher::unregister_notification(
        enumerator,
        crate::utils::ComSend::new(client),
    );

    Ok(())
}

/// Event loop that waits for stop signal.
fn watcher_event_loop(stop_rx: &Receiver<()>) -> Result<()> {
    loop {
        match stop_rx.recv_timeout(Duration::from_millis(500)) {
            Ok(_) | Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => continue,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_watcher_start_and_stop() {
        let (mut watcher, rx) = DeviceWatcher::start().expect("start watcher");

        // We should receive the initial DefaultChanged event
        match rx.recv_timeout(Duration::from_secs(2)) {
            Ok(DeviceEvent::DefaultChanged(_)) => (),
            Ok(other) => panic!("expected DefaultChanged, got {:?}", other),
            Err(e) => panic!("did not receive initial event: {:?}", e),
        }

        watcher.stop();
    }
}
