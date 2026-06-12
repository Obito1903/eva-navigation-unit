//! a310 — Android Auto head unit with a Slint GUI.
//!
//! Architecture:
//!   • Main thread   : Slint event loop (required by most windowing systems)
//!   • Background    : std::thread → tokio::Runtime → android-auto protocol
//!   • Bridge        : mpsc channels + slint::invoke_from_event_loop
//!
//! Run with:
//!   cargo run --release
//!
//! The android-auto dependency is compiled with both the `usb` and `wireless`
//! features (see Cargo.toml).

use bluetooth_rust::{BluetoothAdapterTrait, MessageToBluetoothHost};
use ringbuf::traits::Producer;
use std::{collections::HashSet, sync::Arc};
use tokio::sync::Mutex;

use android_auto::{HeadUnitInfo, VideoConfiguration};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

mod nmrs_extensions;

slint::include_modules!();

// ─── Async ↔ UI message types ────────────────────────────────────────────────

enum MessageFromAsync {
    VideoData {
        data: Vec<u8>,
        _timestamp: Option<u64>,
    },
    Connected,
    Disconnected,
    ExitContainer,
}

enum MessageToAsync {
    AndroidAutoMessage(android_auto::SendableAndroidAutoMessage),
}

// ─── Audio producer type alias ────────────────────────────────────────────────

type AudioProducer = ringbuf::HeapProd<i16>;

// ─── AndroidAutoInner ─────────────────────────────────────────────────────────

struct AndroidAutoInner {
    relay: Option<tokio::task::JoinHandle<()>>,
    connected: bool,
    send: tokio::sync::mpsc::Sender<MessageFromAsync>,
    arecv: Option<tokio::sync::mpsc::Receiver<android_auto::SendableAndroidAutoMessage>>,
    android_send: tokio::sync::mpsc::Sender<android_auto::SendableAndroidAutoMessage>,
    audio_input: Option<cpal::Device>,
    media_stream: Option<(AudioProducer, cpal::Stream)>,
    sys_stream: Option<(AudioProducer, cpal::Stream)>,
    speech_stream: Option<(AudioProducer, cpal::Stream)>,
    input_stream: Option<cpal::Stream>,
}

// ─── AndroidAuto (the protocol handler implementing all AA traits) ─────────────

#[derive(Clone)]
struct AndroidAuto {
    inner: Arc<Mutex<AndroidAutoInner>>,
    config: VideoConfiguration,
    blue: android_auto::BluetoothInformation,
    bluetooth: Arc<bluetooth_rust::BluetoothAdapter>,
    network: Arc<android_auto::NetworkInformation>,
    sensors: android_auto::SensorInformation,
    input_config: android_auto::InputConfiguration,
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
        Some(Arc::new(self.clone()))
    }
}

// ─── Constructor & start ──────────────────────────────────────────────────────

impl AndroidAuto {
    fn new(
        mut recv: tokio::sync::mpsc::Receiver<MessageToAsync>,
        send: tokio::sync::mpsc::Sender<MessageFromAsync>,
        bluetooth: Arc<bluetooth_rust::BluetoothAdapter>,
        blue_address: String,
        network: android_auto::NetworkInformation,
        android_recv: tokio::sync::mpsc::Receiver<android_auto::SendableAndroidAutoMessage>,
        android_send: tokio::sync::mpsc::Sender<android_auto::SendableAndroidAutoMessage>,
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

        let (ai, media_stream, sys_stream, speech_stream) =
            build_audio_streams();

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
            config: VideoConfiguration {
                resolution: android_auto::Wifi::video_resolution::Enum::_480p,
                fps: android_auto::Wifi::video_fps::Enum::_30,
                dpi: 111,
            },
            sensors: android_auto::SensorInformation { sensors },
            input_config: android_auto::InputConfiguration {
                keycodes: vec![1, 2, 3, 4, 5],
                touchscreen: Some((800, 480)),
            },
        }
    }

    async fn start_android_auto(
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

// ─── Audio stream helper ──────────────────────────────────────────────────────

fn build_output_stream_for(
    device: &cpal::Device,
    rate: u32,
    channels: u16,
    buf_size: usize,
) -> Option<(AudioProducer, cpal::Stream)> {
    let configs = device.supported_output_configs().ok()?;
    for c in configs {
        if c.min_sample_rate() <= rate
            && c.max_sample_rate() >= rate
            && c.channels() == channels
            && c.sample_format() == cpal::SampleFormat::I16
        {
            let sc = c.try_with_sample_rate(rate)?;
            let rb = ringbuf::HeapRb::new(buf_size);
            let (producer, mut consumer) = ringbuf::traits::Split::split(rb);
            let stream = device
                .build_output_stream(
                    &sc.config(),
                    move |data: &mut [i16], _| {
                        let mut idx = 0;
                        while idx < data.len() {
                            let n = ringbuf::traits::Consumer::pop_slice(
                                &mut consumer,
                                &mut data[idx..],
                            );
                            if n == 0 {
                                break;
                            }
                            idx += n;
                        }
                    },
                    |err| log::error!("Audio output error: {err:?}"),
                    None,
                )
                .ok()?;
            return Some((producer, stream));
        }
    }
    None
}

fn build_audio_streams() -> (
    Option<cpal::Device>,
    Option<(AudioProducer, cpal::Stream)>,
    Option<(AudioProducer, cpal::Stream)>,
    Option<(AudioProducer, cpal::Stream)>,
) {
    let host = cpal::default_host();
    let ai = host.default_input_device();
    let ao = host.default_output_device();

    if let Some(ao) = &ao {
        let media = build_output_stream_for(ao, 48000, 2, 48000);
        let sys = build_output_stream_for(ao, 16000, 1, 16000);
        let speech = build_output_stream_for(ao, 16000, 1, 16000);
        (ai, media, sys, speech)
    } else {
        (ai, None, None, None)
    }
}

// ─── AndroidAutoContainer ─────────────────────────────────────────────────────

struct AndroidAutoContainer {
    thread: Option<std::thread::JoinHandle<Result<(), String>>>,
    recv: tokio::sync::mpsc::Receiver<MessageFromAsync>,
    send: tokio::sync::mpsc::Sender<MessageToAsync>,
    kill: Option<tokio::sync::oneshot::Sender<()>>,
}

impl AndroidAutoContainer {
    fn new(setup: android_auto::AndroidAutoSetup) -> Self {
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
                let wifi = nmrs::NetworkManager::new().await.expect("Wifi not found");
                let wifi_dev = {
                    let devs = wifi.list_wireless_devices().await.unwrap_or_default();
                    devs.into_iter()
                        .find(|d| d.device_type == nmrs::DeviceType::Wifi)
                        .expect("No wifi device found")
                };

                let hotspot_ssid = "Hotspot".to_string();
                let hotspot_psk = "qwertyuiop".to_string();
                nmrs_extensions::start_hotspot(
                    hotspot_ssid.clone(),
                    hotspot_psk.clone(),
                    &wifi_dev.path,
                )
                .await
                .expect("Failed to start wifi hotspot");

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
                                log::info!("Cancel passkey");
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

                // ── Protocol setup ────────────────────────────────────────
                let aauto = tokio::sync::mpsc::channel(50);
                let aa = AndroidAuto::new(
                    to_async.1,
                    from_async.0,
                    bluetooth,
                    bluetooth_address,
                    android_auto::NetworkInformation {
                        ssid: hotspot_ssid,
                        psk: hotspot_psk,
                        mac_addr: wifi_dev.identity.current_mac.clone(),
                        ip: "10.42.0.1".to_string(),
                        port: 5277,
                        security_mode: android_auto::Bluetooth::SecurityMode::WPA2_PERSONAL,
                        ap_type: android_auto::Bluetooth::AccessPointType::STATIC,
                    },
                    aauto.1,
                    aauto.0,
                );

                let config = android_auto::AndroidAutoConfiguration {
                    unit: HeadUnitInfo {
                        name: "a310".to_string(),
                        car_model: "a310".to_string(),
                        car_year: "2024".to_string(),
                        car_serial: "00000001".to_string(),
                        left_hand: false,
                        head_manufacturer: "a310".to_string(),
                        head_model: "a310".to_string(),
                        sw_build: "1".to_string(),
                        sw_version: "0.1.0".to_string(),
                        native_media: true,
                        hide_clock: Some(true),
                    },
                    custom_certificate: None,
                };

                tokio::select! {
                    _ = aa.start_android_auto(config, setup) => {
                        log::info!("android-auto protocol exited");
                    }
                    _ = kill.1 => {
                        log::info!("Container killed");
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
        let _ = self.kill.take().map(|s| s.send(()));
        self.thread.take().map(|t| t.join());
    }
}

// ─── main ─────────────────────────────────────────────────────────────────────

fn main() -> Result<(), slint::PlatformError> {
    simple_logger::SimpleLogger::new()
        .with_level(log::LevelFilter::Info)
        .init()
        .unwrap();

    let setup = android_auto::setup();

    let window = AppWindow::new()?;
    let window_weak = window.as_weak();

    // ── Start android-auto in background ──────────────────────────────────────
    let mut container = AndroidAutoContainer::new(setup);
    let send_touch = container.send.clone();

    // ── Touch events: Slint UI → android-auto ─────────────────────────────────
    {
        let send = send_touch.clone();
        window.on_touch_event(move |x, y, kind| {
            let mut i_event = android_auto::Wifi::InputEventIndication::new();
            let timestamp: u64 = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_micros() as u64;
            i_event.set_timestamp(timestamp);

            let mut te = android_auto::Wifi::TouchEvent::new();
            let mut tl = android_auto::Wifi::TouchLocation::new();
            tl.set_x(x as u32);
            tl.set_y(y as u32);
            tl.set_pointer_id(0);
            te.touch_location = vec![tl];

            let action = match kind as i32 {
                0 => android_auto::Wifi::touch_action::Enum::POINTER_DOWN,
                2 => android_auto::Wifi::touch_action::Enum::POINTER_UP,
                _ => android_auto::Wifi::touch_action::Enum::DRAG,
            };
            te.set_touch_action(action);
            i_event.touch_event = android_auto::protobuf::MessageField::some(te);

            let msg = android_auto::AndroidAutoMessage::Input(i_event);
            let _ = send.blocking_send(MessageToAsync::AndroidAutoMessage(msg.sendable()));
        });
    }

    // ── Poll async → UI messages with a 16 ms timer ───────────────────────────
    let window_weak2 = window_weak.clone();
    let mut decoder = openh264::decoder::Decoder::new().unwrap();

    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(16),
        move || {
            let Some(win) = window_weak2.upgrade() else {
                return;
            };

            // Drain all pending messages this tick
            while let Ok(msg) = container.recv.try_recv() {
                match msg {
                    MessageFromAsync::ExitContainer => {
                        log::info!("Container exited — restarting");
                        container = AndroidAutoContainer::new(setup);
                    }
                    MessageFromAsync::Connected => {
                        log::info!("Android Auto connected");
                        win.set_aa_connected(true);
                    }
                    MessageFromAsync::Disconnected => {
                        log::info!("Android Auto disconnected");
                        win.set_aa_connected(false);
                        let _ = decoder.flush_remaining();
                    }
                    MessageFromAsync::VideoData { data, .. } => {
                        // Decode H.264 NAL units → RGB → SharedPixelBuffer
                        let mut units = openh264::nal_units(&data).peekable();
                        while let Some(nal) = units.next() {
                            match decoder.decode(nal) {
                                Ok(Some(yuv)) => {
                                    use openh264::formats::YUVSource;
                                    let (w, h) = yuv.dimensions_uv();
                                    let (w, h) = (w * 2, h * 2);
                                    let mut buf =
                                        slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(
                                            w as u32,
                                            h as u32,
                                        );
                                    let rgb_len = yuv.rgb8_len();
                                    let mut rgb_raw = vec![0u8; rgb_len];
                                    yuv.write_rgb8(&mut rgb_raw);
                                    let pixels = buf.make_mut_bytes();
                                    pixels.copy_from_slice(&rgb_raw);
                                    let image = slint::Image::from_rgb8(buf);
                                    win.set_video_frame(image);
                                }
                                Ok(None) => {}
                                Err(e) => {
                                    log::error!("Video decode error: {e:?}");
                                }
                            }
                        }
                    }
                }
            }
        },
    );

    window.run()
}
