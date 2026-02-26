//! Audio device change watcher.
//!
//! This module provides functionality to monitor changes in audio devices,
//! such as device addition, removal, state changes, and default device changes.
//! It uses Windows COM APIs to register for notifications and forwards events
//! through a channel for easy consumption by other parts of the application.

use anyhow::Result;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use crate::com_service::device::{DeviceInfo, get_default_output_device};

#[cfg(target_os = "windows")]
use windows::core::implement;

/// Simple notification that indicates device list or default device changed.
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
    /// Something changed in device topology (add/remove/state). Subscriber should call `list_output_devices()` to get details.
    Changed,
    /// Default device changed; contains current default device.
    DefaultChanged(DeviceInfo),
}

#[cfg(target_os = "windows")]
#[implement(windows::Win32::Media::Audio::IMMNotificationClient)]
pub struct NotificationClient {
    pub sender: std::sync::mpsc::Sender<DeviceEvent>,
}

#[cfg(target_os = "windows")]
impl NotificationClient {
    pub fn new(sender: std::sync::mpsc::Sender<DeviceEvent>) -> Self {
        Self { sender }
    }
}

#[cfg(target_os = "windows")]
impl windows::Win32::Media::Audio::IMMNotificationClient_Impl for NotificationClient {
    fn OnDeviceStateChanged(
        &self,
        _pwstrdeviceid: &windows::core::PCWSTR, // Device ID
        _dwnewstate: u32,                       // New device state
    ) -> windows::core::Result<()> {
        {
            let _ = self.sender.send(DeviceEvent::Changed);
            Ok(())
        }
    }
    fn OnDeviceAdded(&self, _pwstrdeviceid: &windows::core::PCWSTR) -> windows::core::Result<()> {
        {
            let _ = self.sender.send(DeviceEvent::Changed);
            Ok(())
        }
    }
    fn OnDeviceRemoved(&self, _pwstrdeviceid: &windows::core::PCWSTR) -> windows::core::Result<()> {
        {
            let _ = self.sender.send(DeviceEvent::Changed);
            Ok(())
        }
    }
    fn OnDefaultDeviceChanged(
        &self,
        _flow: windows::Win32::Media::Audio::EDataFlow, // eRender, eCapture, eAll
        _role: windows::Win32::Media::Audio::ERole,     // eConsole, eMultimedia, eCommunications
        _pwstrdefaultdeviceid: &windows::core::PCWSTR,  // Default device ID
    ) -> windows::core::Result<()> {
        {
            match get_default_output_device() {
                Ok(d) => {
                    let _ = self.sender.send(DeviceEvent::DefaultChanged(d));
                }
                Err(e) => log::error!("get_default_output_device failed in callback: {:?}", e),
            }
            Ok(())
        }
    }
    fn OnPropertyValueChanged(
        &self,
        _pwstrdeviceid: &windows::core::PCWSTR, // Device ID
        _key: &windows::Win32::UI::Shell::PropertiesSystem::PROPERTYKEY, // Property key
    ) -> windows::core::Result<()> {
        {
            let _ = self.sender.send(DeviceEvent::Changed);
            Ok(())
        }
    }
}

/// Handle for the device watcher. Drop or call `stop()` to unregister and stop the background thread.
pub struct DeviceWatcher {
    stop_tx: Option<Sender<()>>,
    join_handle: Option<std::thread::JoinHandle<()>>,
}

impl DeviceWatcher {
    /// Start a Windows COM-based device watcher and return the watcher handle and a `Receiver` for events.
    ///
    /// This function spawns a background thread that registers for audio device notifications
    /// and forwards events through the returned channel. An initial `DefaultChanged` event
    /// is sent immediately with the current default device.
    ///
    /// # Returns
    /// A tuple containing the `DeviceWatcher` handle and a `Receiver<DeviceEvent>` for events.
    ///
    /// # Errors
    /// Returns an error if COM initialization, enumerator creation, or notification registration fails.
    pub fn start() -> Result<(DeviceWatcher, Receiver<DeviceEvent>)> {
        #[cfg(target_os = "windows")]
        {
            use std::sync::mpsc::RecvTimeoutError;

            // event channel for COM callback thread to notify about device changes
            let (event_tx, event_rx) = mpsc::channel::<DeviceEvent>();
            // channel to signal the COM callback thread to stop and exit. We use a channel instead of an atomic flag for simpler thread wakeup.
            let (stop_tx, stop_rx) = mpsc::channel::<()>();

            // Spawn thread which registers IMMNotificationClient and forwards events to `event_tx`.
            let join_handle = thread::spawn(move || {
                let res = (|| -> Result<()> {
                    let enumerator = crate::com_service::watcher::create_enumerator()?;

                    // Create the COM object instance
                    let client: windows::Win32::Media::Audio::IMMNotificationClient =
                        NotificationClient::new(event_tx.clone()).into();

                    // Register for notifications
                    // We use COM environment to perform the registration
                    crate::com_service::watcher::register_notification(
                        enumerator.clone(),
                        crate::utils::ComSend::new(client.clone()),
                    )?;

                    // Send an initial default-changed event so consumers can fetch current default
                    if let Ok(d) = crate::com_service::device::get_default_output_device() {
                        event_tx.send(DeviceEvent::DefaultChanged(d))?;
                    }

                    // Wait until stop or unexpected errors. Use timeout loop to be able to wake up periodically
                    loop {
                        match stop_rx.recv_timeout(Duration::from_millis(500)) {
                            Ok(_) => break,
                            Err(RecvTimeoutError::Timeout) => continue,
                            Err(RecvTimeoutError::Disconnected) => break,
                        }
                    }

                    // Unregister callback
                    let _ = crate::com_service::watcher::unregister_notification(
                        enumerator,
                        crate::utils::ComSend::new(client),
                    );
                    Ok(())
                })();

                if let Err(e) = res {
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
    }

    /// Stop the watcher and wait for the background thread to exit.
    ///
    /// This method signals the background thread to stop, unregisters the notification client,
    /// and waits for the thread to join. After calling this, no more events will be sent
    /// through the channel.
    ///
    /// # Note
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

#[cfg(test)]
mod tests {
    use super::*;

    // Simple integration test that starts the watcher, waits a short while and then stops.
    // This is a Windows-only integration smoke test; it just ensures the watcher can start and stop.
    #[cfg(target_os = "windows")]
    #[test]
    fn start_and_stop_watcher() {
        let (mut watcher, rx) = DeviceWatcher::start().expect("start watcher");
        // We should receive at least the initial DefaultChanged event
        match rx.recv_timeout(Duration::from_secs(2)) {
            Ok(e) => match e {
                DeviceEvent::DefaultChanged(_) => (),
                _ => panic!("expected DefaultChanged"),
            },
            Err(e) => panic!("did not receive initial event: {:?}", e),
        }
        watcher.stop();
    }
}
