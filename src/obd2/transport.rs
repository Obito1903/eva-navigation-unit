//! Bluetooth RFCOMM transport for `obd2-core`, built directly on
//! `bluetooth-rust`'s client RFCOMM socket support (Serial Port Profile)
//! rather than obd2-core's own serial/BLE transports — this avoids adding a
//! second Bluetooth stack dependency, since `bluetooth-rust` is already used
//! for Android Auto pairing (see `crate::container`).

use async_trait::async_trait;
use bluetooth_rust::{
    BluetoothAdapterBuilder, BluetoothAdapterTrait, BluetoothDeviceTrait, BluetoothSocketTrait,
    BluetoothUuid, MessageToBluetoothHost, ResponseToPasskey,
};
use obd2_core::error::Obd2Error;
use obd2_core::transport::Transport;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// The conventional RFCOMM channel used by most ELM327 Serial Port Profile
/// adapters, used as a fallback when SDP-based channel discovery fails.
const FALLBACK_SPP_CHANNEL: u8 = 1;

/// A connected Bluetooth RFCOMM socket to a paired ELM327 adapter, wired up
/// as an `obd2_core::transport::Transport`.
pub(super) struct BluetoothRfcommTransport {
    socket: bluetooth_rust::BluetoothSocket,
}

impl BluetoothRfcommTransport {
    /// Connect to the paired device at `device_address` (a Bluetooth MAC,
    /// e.g. `"AA:BB:CC:DD:EE:FF"`) over RFCOMM/SPP.
    ///
    /// Builds its own dedicated Bluetooth adapter handle, independent of the
    /// one `AndroidAutoContainer` uses for the phone connection — the ELM327
    /// is a logically separate peer, and this keeps OBD2 connectivity
    /// decoupled from the Android Auto session lifecycle.
    pub(super) async fn connect(device_address: &str) -> Result<Self, String> {
        let (pairing_tx, mut pairing_rx) = tokio::sync::mpsc::channel(5);
        let mut builder = BluetoothAdapterBuilder::new();
        builder.with_sender(pairing_tx);
        let adapter = builder
            .async_build()
            .await
            .map_err(|e| format!("failed to open Bluetooth adapter: {e}"))?;

        // The ELM327 is expected to already be paired, so no passkey prompt
        // should normally occur — auto-accept anyway rather than stalling.
        tokio::spawn(async move {
            while let Some(msg) = pairing_rx.recv().await {
                match msg {
                    MessageToBluetoothHost::DisplayPasskey(_, sender)
                    | MessageToBluetoothHost::ConfirmPasskey(_, sender) => {
                        let _ = sender.send(ResponseToPasskey::Yes).await;
                    }
                    MessageToBluetoothHost::CancelDisplayPasskey => {}
                }
            }
        });

        let async_adapter = adapter
            .supports_async()
            .ok_or_else(|| "Bluetooth adapter does not support async operations".to_string())?;

        let devices = async_adapter
            .get_paired_devices()
            .await
            .ok_or_else(|| "no paired Bluetooth devices found".to_string())?;

        let mut device = devices
            .into_iter()
            .find_map(|mut d| match d.get_address() {
                Ok(addr) if addr.eq_ignore_ascii_case(device_address) => Some(d),
                _ => None,
            })
            .ok_or_else(|| format!("paired device {device_address} not found"))?;

        // SDP-based RFCOMM channel discovery for the Serial Port Profile,
        // falling back to the conventional channel 1 if it fails. This runs
        // a short blocking raw-socket query, acceptable since it only
        // happens once per (re)connect, not on the polling hot path.
        let channel = device
            .run_sdp(BluetoothUuid::SPP)
            .ok()
            .and_then(|record| record.rfcomm_channel())
            .unwrap_or(FALLBACK_SPP_CHANNEL);

        let mut socket = device
            .get_rfcomm_socket(channel, false)
            .map_err(|e| format!("failed to create RFCOMM socket: {e}"))?;
        socket
            .async_connect()
            .await
            .map_err(|e| format!("failed to connect RFCOMM socket: {e}"))?;

        Ok(Self { socket })
    }
}

#[async_trait]
impl Transport for BluetoothRfcommTransport {
    async fn write(&mut self, data: &[u8]) -> Result<(), Obd2Error> {
        let stream = self
            .socket
            .supports_async()
            .ok_or_else(|| Obd2Error::Transport("Bluetooth socket has no async I/O".into()))?;
        stream.write_all(data).await?;
        stream.flush().await?;
        Ok(())
    }

    async fn read(&mut self) -> Result<Vec<u8>, Obd2Error> {
        let stream = self
            .socket
            .supports_async()
            .ok_or_else(|| Obd2Error::Transport("Bluetooth socket has no async I/O".into()))?;
        // ELM327 terminates every response with a trailing `>` prompt
        // character; RFCOMM otherwise has no message framing, so read until
        // we see it (or hit a generous safety cap against a runaway stream).
        let mut buf = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            let n = stream.read(&mut byte).await?;
            if n == 0 {
                break; // socket closed
            }
            buf.push(byte[0]);
            if byte[0] == b'>' || buf.len() > 4096 {
                break;
            }
        }
        Ok(buf)
    }

    async fn reset(&mut self) -> Result<(), Obd2Error> {
        // No in-place reset for a live RFCOMM socket; the worker reconnects
        // from scratch on error instead (see `crate::obd2::worker`).
        Ok(())
    }

    fn name(&self) -> &str {
        "bluetooth-rfcomm"
    }
}
