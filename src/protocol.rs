//! The android-auto protocol handler: the [`AndroidAuto`] type and its
//! implementations of every android-auto trait (wireless, bluetooth, video,
//! sensors, audio in/out, input, wired and the main trait).

use std::{collections::HashSet, sync::Arc};

use bluetooth_rust::BluetoothAdapterTrait;
use cpal::traits::{DeviceTrait, StreamTrait};
use ringbuf::traits::Producer;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Mutex;

use android_auto::VideoConfiguration;

use crate::audio::{build_audio_streams, AudioProducer};
use crate::messages::{MessageFromAsync, MessageToAsync};

/// Mutable, shared protocol state guarded by an async mutex.
pub(crate) struct AndroidAutoInner {
    pub(crate) relay: Option<tokio::task::JoinHandle<()>>,
    pub(crate) connected: bool,
    pub(crate) send: tokio::sync::mpsc::Sender<MessageFromAsync>,
    pub(crate) arecv: Option<tokio::sync::mpsc::Receiver<android_auto::SendableAndroidAutoMessage>>,
    pub(crate) android_send: tokio::sync::mpsc::Sender<android_auto::SendableAndroidAutoMessage>,
    pub(crate) audio_input: Option<cpal::Device>,
    pub(crate) media_stream: Option<(AudioProducer, cpal::Stream)>,
    pub(crate) sys_stream: Option<(AudioProducer, cpal::Stream)>,
    pub(crate) speech_stream: Option<(AudioProducer, cpal::Stream)>,
    pub(crate) input_stream: Option<cpal::Stream>,
}

/// The protocol handler implementing all android-auto traits.
#[derive(Clone)]
pub(crate) struct AndroidAuto {
    pub(crate) inner: Arc<Mutex<AndroidAutoInner>>,
    pub(crate) config: VideoConfiguration,
    pub(crate) blue: android_auto::BluetoothInformation,
    pub(crate) bluetooth: Arc<bluetooth_rust::BluetoothAdapter>,
    pub(crate) network: Arc<android_auto::NetworkInformation>,
    pub(crate) sensors: android_auto::SensorInformation,
    pub(crate) input_config: android_auto::InputConfiguration,
    /// Whether USB (wired) Android Auto is enabled. When `false`,
    /// [`supports_wired`](AndroidAuto::supports_wired) returns `None` so the
    /// worker never starts the USB connection path.
    pub(crate) usb_enabled: Arc<AtomicBool>,
}

// ── Wireless traits ───────────────────────────────────────────────────────────

#[async_trait::async_trait]
impl android_auto::AndroidAutoWirelessTrait for AndroidAuto {
    async fn setup_bluetooth_profile(
        &self,
        suggestions: &bluetooth_rust::BluetoothRfcommProfileSettings,
    ) -> Result<bluetooth_rust::BluetoothRfcommProfileAsync, String> {
        if let Some(b) = self.bluetooth.supports_async() {
            b.register_rfcomm_profile(suggestions.clone()).await
        } else {
            Err("Async not supported".to_string())
        }
    }

    fn get_wifi_details(&self) -> android_auto::NetworkInformation {
        self.network.as_ref().to_owned()
    }
}

#[async_trait::async_trait]
impl android_auto::AndroidAutoBluetoothTrait for AndroidAuto {
    async fn do_stuff(&self) {}

    fn get_config(&self) -> &android_auto::BluetoothInformation {
        &self.blue
    }
}

// ── Video ─────────────────────────────────────────────────────────────────────

#[async_trait::async_trait]
impl android_auto::AndroidAutoVideoChannelTrait for AndroidAuto {
    async fn receive_video(&self, data: Vec<u8>, timestamp: Option<u64>) {
        let i = self.inner.lock().await;
        let _ = i
            .send
            .send(MessageFromAsync::VideoData {
                data,
                _timestamp: timestamp,
            })
            .await;
    }

    async fn setup_video(&self) -> Result<(), ()> {
        Ok(())
    }

    async fn teardown_video(&self) {}

    async fn wait_for_focus(&self) {}

    async fn set_focus(&self, _focus: bool) {}

    fn retrieve_video_configuration(&self) -> &VideoConfiguration {
        &self.config
    }
}

// ── Sensors ───────────────────────────────────────────────────────────────────

#[async_trait::async_trait]
impl android_auto::AndroidAutoSensorTrait for AndroidAuto {
    fn get_supported_sensors(&self) -> &android_auto::SensorInformation {
        &self.sensors
    }

    async fn start_sensor(
        &self,
        stype: android_auto::Wifi::sensor_type::Enum,
    ) -> Result<(), ()> {
        if self.sensors.sensors.contains(&stype) {
            let mut m3 = android_auto::Wifi::SensorEventIndication::new();
            match stype {
                android_auto::Wifi::sensor_type::Enum::DRIVING_STATUS => {
                    let mut ds = android_auto::Wifi::DrivingStatus::new();
                    ds.set_status(
                        android_auto::Wifi::DrivingStatusEnum::UNRESTRICTED as i32,
                    );
                    m3.driving_status.push(ds);
                }
                android_auto::Wifi::sensor_type::Enum::NIGHT_DATA => {
                    let mut ds = android_auto::Wifi::NightMode::new();
                    ds.set_is_night(false);
                    m3.night_mode.push(ds);
                }
                _ => return Err(()),
            }
            let s = self.inner.lock().await;
            let m = android_auto::AndroidAutoMessage::Sensor(m3);
            s.android_send.send(m.sendable()).await.map_err(|_| ())?;
            Ok(())
        } else {
            Err(())
        }
    }
}

// ── Audio output ──────────────────────────────────────────────────────────────

#[async_trait::async_trait]
impl android_auto::AndroidAutoAudioOutputTrait for AndroidAuto {
    async fn open_output_channel(
        &self,
        _t: android_auto::AudioChannelType,
    ) -> Result<(), ()> {
        Ok(())
    }

    async fn close_output_channel(
        &self,
        _t: android_auto::AudioChannelType,
    ) -> Result<(), ()> {
        Ok(())
    }

    async fn receive_output_audio(
        &self,
        t: android_auto::AudioChannelType,
        data: Vec<u8>,
    ) {
        let mut s = self.inner.lock().await;
        let r2: Vec<i16> = data
            .chunks_exact(2)
            .map(|v| i16::from_le_bytes([v[0], v[1]]))
            .collect();
        match t {
            android_auto::AudioChannelType::Media => {
                s.media_stream.as_mut().map(|m| m.0.push_slice(&r2));
            }
            android_auto::AudioChannelType::System => {
                s.sys_stream.as_mut().map(|m| m.0.push_slice(&r2));
            }
            android_auto::AudioChannelType::Speech => {
                s.speech_stream.as_mut().map(|m| m.0.push_slice(&r2));
            }
        }
    }

    async fn start_output_audio(&self, t: android_auto::AudioChannelType) {
        let s = self.inner.lock().await;
        match t {
            android_auto::AudioChannelType::Media => {
                s.media_stream.as_ref().map(|m| m.1.play());
            }
            android_auto::AudioChannelType::System => {
                s.sys_stream.as_ref().map(|m| m.1.play());
            }
            android_auto::AudioChannelType::Speech => {
                s.speech_stream.as_ref().map(|m| m.1.play());
            }
        }
    }

    async fn stop_output_audio(&self, t: android_auto::AudioChannelType) {
        let s = self.inner.lock().await;
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        match t {
            android_auto::AudioChannelType::Media => {
                s.media_stream.as_ref().map(|m| m.1.pause());
            }
            android_auto::AudioChannelType::System => {
                s.sys_stream.as_ref().map(|m| m.1.pause());
            }
            android_auto::AudioChannelType::Speech => {
                s.speech_stream.as_ref().map(|m| m.1.pause());
            }
        }
    }
}

// ── Audio input ───────────────────────────────────────────────────────────────

#[async_trait::async_trait]
impl android_auto::AndroidAutoAudioInputTrait for AndroidAuto {
    async fn open_input_channel(&self) -> Result<(), ()> {
        let mut s = self.inner.lock().await;
        let config = cpal::StreamConfig {
            channels: 1,
            sample_rate: 16000,
            buffer_size: cpal::BufferSize::Default,
        };
        if let Some(ai) = &s.audio_input {
            let android_send = s.android_send.clone();
            if let Ok(str) = ai.build_input_stream(
                &config,
                move |data: &[i16], _| {
                    let bytes: Vec<u8> =
                        data.iter().flat_map(|s| s.to_le_bytes()).collect();
                    let timestamp = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_micros() as u64;
                    let msg =
                        android_auto::AndroidAutoMessage::Audio(Some(timestamp), bytes);
                    if let Err(e) = android_send.try_send(msg.sendable()) {
                        log::warn!("Dropped audio input frame: {:?}", e);
                    }
                },
                |err| log::error!("Audio input error: {:?}", err),
                None,
            ) {
                let _ = str.play();
                s.input_stream = Some(str);
            } else {
                log::error!("Failed to open input stream");
            }
        }
        Ok(())
    }

    async fn close_input_channel(&self) -> Result<(), ()> {
        let mut s = self.inner.lock().await;
        s.input_stream.take();
        Ok(())
    }

    async fn start_input_audio(&self) {}

    async fn audio_input_ack(
        &self,
        chan: u8,
        ack: android_auto::Wifi::AVMediaAckIndication,
    ) {
        log::info!("Ack audio input chan={chan} {ack:?}");
    }

    async fn stop_input_audio(&self) {
        let mut s = self.inner.lock().await;
        s.input_stream.take();
    }
}

// ── Input channel ─────────────────────────────────────────────────────────────

#[async_trait::async_trait]
impl android_auto::AndroidAutoInputChannelTrait for AndroidAuto {
    async fn binding_request(&self, _code: u32) -> Result<(), ()> {
        Ok(())
    }

    fn retrieve_input_configuration(&self) -> &android_auto::InputConfiguration {
        &self.input_config
    }
}

// ── USB wired ─────────────────────────────────────────────────────────────────

#[async_trait::async_trait]
impl android_auto::AndroidAutoWiredTrait for AndroidAuto {}

// ── Main trait ────────────────────────────────────────────────────────────────

#[async_trait::async_trait]
impl android_auto::AndroidAutoMainTrait for AndroidAuto {
    async fn connect(&self) {
        let mut i = self.inner.lock().await;
        let _ = i.send.send(MessageFromAsync::Connected).await;
        i.connected = true;
    }

    async fn disconnect(&self) {
        let mut s = self.inner.lock().await;
        let _ = s.send.send(MessageFromAsync::Disconnected).await;
        s.connected = false;
    }

    async fn get_receiver(
        &self,
    ) -> Option<tokio::sync::mpsc::Receiver<android_auto::SendableAndroidAutoMessage>> {
        let mut s = self.inner.lock().await;
        s.arecv.take()
    }

    fn supports_bluetooth(&self) -> Option<&dyn android_auto::AndroidAutoBluetoothTrait> {
        Some(self)
    }

    fn supports_wireless(&self) -> Option<Arc<dyn android_auto::AndroidAutoWirelessTrait>> {
        Some(Arc::new(self.clone()))
    }

    fn supports_wired(&self) -> Option<Arc<dyn android_auto::AndroidAutoWiredTrait>> {
        if self.usb_enabled.load(Ordering::Relaxed) {
            Some(Arc::new(self.clone()))
        } else {
            None
        }
    }
}

// ── Constructor & start ─────────────────────────────────────────────────────

/// Build a [`VideoConfiguration`] for the requested vertical resolution
/// (`480`, `720` or `1080`) and frame rate (`30` or `60`), sizing the active
/// picture to match the head unit's screen aspect ratio via margins.
///
/// Android Auto encodes at fixed 16:9 base resolutions (480p = 800×480,
/// 720p = 1280×720, 1080p = 1920×1080). When the screen is not 16:9 we keep
/// the base buffer and add margins so the visible region matches
/// `screen_w / screen_h`.
pub(crate) fn build_video_configuration(
    resolution: i32,
    fps: i32,
    screen_w: u32,
    screen_h: u32,
    dpi: u16,
) -> VideoConfiguration {
    let (res_enum, base_w, base_h) = if resolution >= 1080 {
        (android_auto::Wifi::video_resolution::Enum::_1080p, 1920i32, 1080i32)
    } else if resolution >= 720 {
        (android_auto::Wifi::video_resolution::Enum::_720p, 1280i32, 720i32)
    } else {
        (android_auto::Wifi::video_resolution::Enum::_480p, 800i32, 480i32)
    };

    let fps_enum = if fps >= 60 {
        android_auto::Wifi::video_fps::Enum::_60
    } else {
        android_auto::Wifi::video_fps::Enum::_30
    };

    // Guard against a degenerate/zero screen size; fall back to the 16:9 base.
    let aspect = if screen_w == 0 || screen_h == 0 {
        base_w as f64 / base_h as f64
    } else {
        screen_w as f64 / screen_h as f64
    };

    // Fit the screen aspect inside the base buffer, padding the shorter axis.
    let (margin_width, margin_height) = {
        let target_w = (base_h as f64 * aspect).round() as i32;
        if target_w <= base_w {
            ((base_w - target_w).max(0) as u16, 0u16)
        } else {
            let target_h = (base_w as f64 / aspect).round() as i32;
            (0u16, (base_h - target_h).max(0) as u16)
        }
    };

    let res_label = if resolution >= 1080 {
        1080
    } else if resolution >= 720 {
        720
    } else {
        480
    };
    let fps_label = if fps >= 60 { 60 } else { 30 };
    log::info!(
        "Android Auto video: {res_label}p@{fps_label} base {base_w}x{base_h}, screen {screen_w}x{screen_h}, margins {margin_width}x{margin_height}"
    );

    VideoConfiguration {
        resolution: res_enum,
        fps: fps_enum,
        dpi,
        margin_width,
        margin_height,
    }
}

impl AndroidAuto {
    pub(crate) fn new(
        mut recv: tokio::sync::mpsc::Receiver<MessageToAsync>,
        send: tokio::sync::mpsc::Sender<MessageFromAsync>,
        bluetooth: Arc<bluetooth_rust::BluetoothAdapter>,
        blue_address: String,
        network: android_auto::NetworkInformation,
        android_recv: tokio::sync::mpsc::Receiver<android_auto::SendableAndroidAutoMessage>,
        android_send: tokio::sync::mpsc::Sender<android_auto::SendableAndroidAutoMessage>,
        video_config: VideoConfiguration,
        usb_enabled: Arc<AtomicBool>,
    ) -> Self {
        let mut sensors = HashSet::new();
        sensors.insert(android_auto::Wifi::sensor_type::Enum::DRIVING_STATUS);
        sensors.insert(android_auto::Wifi::sensor_type::Enum::NIGHT_DATA);

        let android_send2 = android_send.clone();
        let relay = tokio::spawn(async move {
            'main_loop: loop {
                while let Some(m) = recv.recv().await {
                    match m {
                        MessageToAsync::AndroidAutoMessage(msg) => {
                            if let Err(e) = android_send2.send(msg).await {
                                log::error!("Relay error: {e:?}");
                                break 'main_loop;
                            }
                        }
                    }
                }
            }
        });

        let (ai, media_stream, sys_stream, speech_stream) = build_audio_streams();

        Self {
            inner: Arc::new(Mutex::new(AndroidAutoInner {
                relay: Some(relay),
                connected: false,
                send,
                arecv: Some(android_recv),
                android_send,
                audio_input: ai,
                media_stream,
                sys_stream,
                speech_stream,
                input_stream: None,
            })),
            bluetooth,
            network: Arc::new(network),
            blue: android_auto::BluetoothInformation {
                address: blue_address,
            },
            config: video_config,
            sensors: android_auto::SensorInformation { sensors },
            input_config: android_auto::InputConfiguration {
                keycodes: vec![1, 2, 3, 4, 5],
                touchscreen: Some((800, 480)),
            },
            usb_enabled,
        }
    }

    pub(crate) async fn start_android_auto(
        self,
        config: android_auto::AndroidAutoConfiguration,
        setup: android_auto::AndroidAutoSetup,
    ) -> Result<(), String> {
        let mut joinset = tokio::task::JoinSet::new();
        let relay = {
            let mut s = self.inner.lock().await;
            s.relay.take()
        };
        use android_auto::AndroidAutoMainTrait;
        let b = Box::new(self);
        let a = b.run(config, &mut joinset, &setup).await;
        joinset.join_all().await;
        relay.map(|r| r.abort());
        a
    }
}
