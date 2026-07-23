//! Owns the background thread + tokio runtime that drives the android-auto
//! protocol, and the channels bridging it to the UI thread.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};

use bluetooth_rust::{BluetoothAdapterTrait, MessageToBluetoothHost};

use android_auto::HeadUnitInfo;

use crate::messages::{MessageFromAsync, MessageToAsync};
#[cfg(feature = "networkmanager-hotspot")]
use crate::nmrs_extensions;
use crate::protocol::AndroidAuto;

/// Android Auto video settings shared with the UI thread. Read whenever the
/// worker (re)builds the protocol so changes take effect on the next
/// connection.
pub(crate) struct VideoSettings {
    /// Vertical resolution lines (720 or 1080).
    pub(crate) resolution: AtomicI32,
    /// Frame rate (30 or 60 fps).
    pub(crate) fps: AtomicI32,
    /// Android Auto display density in DPI.
    pub(crate) dpi: AtomicI32,
    /// Current screen width used to derive the picture aspect ratio.
    pub(crate) screen_w: AtomicU32,
    /// Current screen height used to derive the picture aspect ratio.
    pub(crate) screen_h: AtomicU32,
}

/// Holds the worker thread and the channels used to communicate with it.
pub(crate) struct AndroidAutoContainer {
    thread: Option<std::thread::JoinHandle<Result<(), String>>>,
    pub(crate) recv: tokio::sync::mpsc::Receiver<MessageFromAsync>,
    pub(crate) send: tokio::sync::mpsc::Sender<MessageToAsync>,
    kill: Option<tokio::sync::oneshot::Sender<()>>,
}

impl AndroidAutoContainer {
    pub(crate) fn new(
        setup: android_auto::AndroidAutoSetup,
        wireless: Arc<AtomicBool>,
        usb: Arc<AtomicBool>,
        reset_stale_accessory: bool,
        video: Arc<VideoSettings>,
        hotspot_backend: Arc<AtomicI32>,
        hotspot_channel: Arc<AtomicI32>,
    ) -> Self {
        let to_async = tokio::sync::mpsc::channel(50);
        let from_async = tokio::sync::mpsc::channel(50);
        let kill = tokio::sync::oneshot::channel::<()>();

        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to build tokio runtime");

        let send_exit = from_async.0.clone();
        let thread = std::thread::spawn(move || {
            let r = rt.block_on(async {
                // ── Wireless setup ────────────────────────────────────────
                // Only touch wifi hardware when wireless Android Auto is
                // enabled. In USB-only mode the device may not exist at all,
                // and requiring it here would abort the worker (and USB too).
                let hotspot_ssid = "Hotspot".to_string();
                let hotspot_psk = "qwertyuiop".to_string();
                // Keeps the hostapd/dnsmasq processes alive for the lifetime of
                // this worker; dropping it tears the AP down. `None` for the
                // NetworkManager backend (NM owns the AP connection instead).
                let mut _hotspot_guard: Option<crate::hostapd::HostapdHandle> = None;
                let wifi_mac = if wireless.load(Ordering::Relaxed) {
                    // Wireless setup must never abort the worker: some devices
                    // have wifi hardware that cannot host an AP (e.g. Broadcom
                    // FullMAC `brcmfmac` chips whose firmware lacks AP mode).
                    // If anything here fails we log a warning and fall back to
                    // USB-only Android Auto instead of panicking.
                    let backend = hotspot_backend.load(Ordering::Relaxed);
                    let channel = hotspot_channel.load(Ordering::Relaxed);
                    match async {
                        let wifi = nmrs::NetworkManager::new()
                            .await
                            .map_err(|e| format!("NetworkManager unavailable: {e}"))?;
                        let wifi_dev = {
                            let devs = wifi.list_wireless_devices().await.unwrap_or_default();
                            devs.into_iter()
                                .find(|d| d.device_type == nmrs::DeviceType::Wifi)
                                .ok_or_else(|| "No wifi device found".to_string())?
                        };
                        let mac = wifi_dev.identity.current_mac.clone();
                        let guard = if backend == 1 {
                            // hostapd backend: a privileged `eva-hotspot.service`
                            // (started here via systemd + polkit) owns the radio,
                            // so eva-ui itself stays unprivileged. SSID/PSK/channel
                            // are configured in the service, not passed from here;
                            // the config `hotspot_channel` ({channel}) is therefore
                            // ignored for this backend.
                            log::info!(
                                "Starting hotspot via hostapd backend \
                                 (eva-hotspot.service; config hotspot_channel={channel} \
                                 is set in the service, not here)"
                            );
                            let handle = crate::hostapd::HostapdHandle::start()
                                .map_err(|e| format!("hostapd hotspot failed: {e}"))?;
                            Some(handle)
                        } else {
                            // NetworkManager backend (default).
                            #[cfg(feature = "networkmanager-hotspot")]
                            {
                                log::info!("Starting hotspot via NetworkManager backend");
                                nmrs_extensions::start_hotspot(
                                    hotspot_ssid.clone(),
                                    hotspot_psk.clone(),
                                    &wifi_dev.path,
                                )
                                .await
                                .map_err(|e| format!("Failed to start wifi hotspot: {e}"))?;
                                None
                            }
                            // Without the `networkmanager-hotspot` feature this backend
                            // can't be driven; fail the same way any other hotspot setup
                            // failure does, so the caller falls back to USB-only instead
                            // of refusing to compile.
                            #[cfg(not(feature = "networkmanager-hotspot"))]
                            {
                                return Err(
                                    "NetworkManager hotspot backend not compiled into this \
                                     build (rebuild with the networkmanager-hotspot feature)"
                                        .to_string(),
                                );
                            }
                        };
                        Ok::<(String, Option<crate::hostapd::HostapdHandle>), String>((mac, guard))
                    }
                    .await
                    {
                        Ok((mac, guard)) => {
                            _hotspot_guard = guard;
                            mac
                        }
                        Err(e) => {
                            log::warn!(
                                "Wireless Android Auto setup failed ({e}); \
                                 falling back to USB-only"
                            );
                            String::new()
                        }
                    }
                } else {
                    log::debug!("Wireless Android Auto disabled — skipping wifi setup");
                    String::new()
                };

                let (mut bluechan, bluetooth) = {
                    let ch = tokio::sync::mpsc::channel(5);
                    let mut builder = bluetooth_rust::BluetoothAdapterBuilder::new();
                    builder.with_sender(ch.0);
                    let bt = Arc::new(
                        builder
                            .async_build()
                            .await
                            .expect("Could not open bluetooth"),
                    );
                    (ch.1, bt)
                };

                if let Some(b) = bluetooth.supports_async() {
                    b.set_discoverable(true)
                        .await
                        .expect("Failed to make bluetooth discoverable");
                }

                // Handle BT pairing prompts
                tokio::spawn(async move {
                    while let Some(m) = bluechan.recv().await {
                        match m {
                            MessageToBluetoothHost::DisplayPasskey(key, sender) => {
                                log::info!("Passkey: {key}");
                                let _ = sender
                                    .send(bluetooth_rust::ResponseToPasskey::Yes)
                                    .await;
                            }
                            MessageToBluetoothHost::ConfirmPasskey(key, sender) => {
                                log::info!("Confirm passkey: {key}");
                                let _ = sender
                                    .send(bluetooth_rust::ResponseToPasskey::Yes)
                                    .await;
                            }
                            MessageToBluetoothHost::CancelDisplayPasskey => {
                                log::debug!("Cancel passkey");
                            }
                        }
                    }
                });

                let blue_addresses = if let Some(b) = bluetooth.supports_async() {
                    b.addresses().await
                } else {
                    panic!("Async bluetooth not supported");
                };
                let bluetooth_address = blue_addresses
                    .first()
                    .map(|a| match a {
                        bluetooth_rust::BluetoothAdapterAddress::String(s) => {
                            s.to_owned()
                        }
                        bluetooth_rust::BluetoothAdapterAddress::Byte(b) => {
                            format!(
                                "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
                                b[0], b[1], b[2], b[3], b[4], b[5]
                            )
                        }
                    })
                    .expect("No bluetooth hardware found");
                log::info!("Bluetooth adapter ready ({bluetooth_address}); discoverable");

                // ── Protocol setup ────────────────────────────────────────
                let aauto = tokio::sync::mpsc::channel(50);
                let video_config = crate::protocol::build_video_configuration(
                    video.resolution.load(Ordering::Relaxed),
                    video.fps.load(Ordering::Relaxed),
                    video.screen_w.load(Ordering::Relaxed),
                    video.screen_h.load(Ordering::Relaxed),
                    video.dpi.load(Ordering::Relaxed).clamp(1, u16::MAX as i32) as u16,
                );
                let aa = AndroidAuto::new(crate::protocol::AndroidAutoInit {
                    recv: to_async.1,
                    send: from_async.0,
                    bluetooth,
                    blue_address: bluetooth_address,
                    network: android_auto::NetworkInformation {
                        ssid: hotspot_ssid,
                        psk: hotspot_psk,
                        mac_addr: wifi_mac,
                        ip: "10.42.0.1".to_string(),
                        port: 5277,
                        security_mode: android_auto::Bluetooth::SecurityMode::WPA2_PERSONAL,
                        ap_type: android_auto::Bluetooth::AccessPointType::STATIC,
                    },
                    android_recv: aauto.1,
                    android_send: aauto.0,
                    video_config,
                    usb_enabled: usb,
                });

                let config = android_auto::AndroidAutoConfiguration {
                    unit: HeadUnitInfo {
                        name: "eva-navigation-unit".to_string(),
                        car_model: "eva-navigation-unit".to_string(),
                        car_year: "2024".to_string(),
                        car_serial: "00000001".to_string(),
                        left_hand: false,
                        head_manufacturer: "eva-navigation-unit".to_string(),
                        head_model: "eva-navigation-unit".to_string(),
                        sw_build: "1".to_string(),
                        sw_version: "0.1.0".to_string(),
                        native_media: true,
                        hide_clock: Some(true),
                    },
                    custom_certificate: None,
                    reset_stale_accessory,
                };

                tokio::select! {
                    _ = aa.start_android_auto(config, setup) => {
                        log::info!("android-auto protocol exited");
                    }
                    _ = kill.1 => {
                        log::debug!("Container killed");
                    }
                }
                Ok::<(), String>(())
            });

            let _ = send_exit.blocking_send(MessageFromAsync::ExitContainer);
            r
        });

        Self {
            thread: Some(thread),
            recv: from_async.1,
            send: to_async.0,
            kill: Some(kill.0),
        }
    }
}

impl Drop for AndroidAutoContainer {
    fn drop(&mut self) {
        // Signal the worker to stop.
        let _ = self.kill.take().map(|s| s.send(()));

        // Join off the current thread. `Drop` runs on the UI/event-loop thread
        // when the container is replaced on restart, and joining the worker
        // here would block the event loop until the tokio runtime finishes
        // tearing down (bluetooth/USB cleanup) — freezing the UI. Reclaim the
        // thread in the background instead so the event loop never stalls.
        if let Some(thread) = self.thread.take() {
            std::thread::spawn(move || {
                if let Err(e) = thread.join() {
                    log::warn!("android-auto worker thread panicked on shutdown: {e:?}");
                }
            });
        }
    }
}
