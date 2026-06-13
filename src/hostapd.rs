//! Alternative Wi-Fi hotspot backend driven by `hostapd` + `dnsmasq`.
//!
//! The default hotspot backend uses NetworkManager over D-Bus
//! ([`crate::nmrs_extensions`]), which is convenient but inherits NM's
//! limitations: it cannot act as a DFS master and gives little control over
//! the AP's band/channel. `hostapd` talks to the driver directly via nl80211
//! and is the more capable backend for real head-unit hardware (e.g. a radio
//! that can host a non-DFS 5 GHz AP), so it is offered here as a selectable
//! alternative.
//!
//! ## Requirements
//! Unlike the NetworkManager backend, this path needs the `hostapd` and
//! `dnsmasq` binaries installed and the process must be able to run them and
//! reconfigure the interface — i.e. it needs root (or `CAP_NET_ADMIN` plus the
//! relevant binaries allowed). If any step fails the caller logs a warning and
//! falls back to USB-only Android Auto, so an unprivileged run degrades
//! gracefully instead of crashing.
//!
//! ## What it sets up
//! 1. Releases the interface from NetworkManager (`nmcli device set … managed no`)
//!    so NM/wpa_supplicant stop fighting `hostapd` for the radio.
//! 2. Spawns `hostapd` with a generated config (SSID/PSK/band/channel).
//! 3. Assigns the `10.42.0.1/24` gateway address to the interface.
//! 4. Spawns `dnsmasq` to serve DHCP on that subnet so the phone gets an IP.
//!
//! The returned [`HostapdHandle`] tears everything down again on drop.

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Child, Command};

/// Gateway address handed to the phone; matches the value advertised over
/// Bluetooth in [`crate::container`] and the NetworkManager backend.
const GATEWAY_CIDR: &str = "10.42.0.1/24";
/// DHCP pool served by `dnsmasq` on the AP subnet.
const DHCP_RANGE: &str = "10.42.0.10,10.42.0.100,255.255.255.0,12h";
/// Gateway address (without prefix) advertised as router/DNS via DHCP.
const GATEWAY_IP: &str = "10.42.0.1";

/// A running `hostapd` + `dnsmasq` hotspot. Dropping it stops both processes
/// and restores the interface to NetworkManager's control.
pub struct HostapdHandle {
    iface: String,
    hostapd: Child,
    dnsmasq: Option<Child>,
    conf_path: PathBuf,
}

impl HostapdHandle {
    /// Bring up a `hostapd` access point on `iface` and start a DHCP server.
    ///
    /// `channel` selects the Wi-Fi channel: `0` means "pick a sensible 5 GHz
    /// default", channels `1..=14` use the 2.4 GHz band, and `>= 36` use 5 GHz.
    pub fn start(
        iface: &str,
        ssid: &str,
        psk: &str,
        channel: i32,
    ) -> Result<Self, String> {
        // Release the radio from NetworkManager so it stops running
        // wpa_supplicant on the interface; otherwise hostapd cannot own it.
        run_ok(Command::new("nmcli").args(["device", "set", iface, "managed", "no"]))
            .map_err(|e| format!("failed to unmanage {iface} in NetworkManager: {e}"))?;

        // Make sure the interface is administratively up before hostapd binds.
        let _ = run_ok(Command::new("ip").args(["link", "set", iface, "up"]));

        // Write the hostapd config with the PSK to a private (0600) file.
        let conf_path = std::env::temp_dir().join("eva-hostapd.conf");
        let conf = hostapd_conf(iface, ssid, psk, channel);
        write_private(&conf_path, conf.as_bytes())
            .map_err(|e| format!("failed to write hostapd config: {e}"))?;

        // Spawn hostapd. It keeps running in the foreground; we own the child.
        let mut hostapd = Command::new("hostapd")
            .arg(&conf_path)
            .spawn()
            .map_err(|e| format!("failed to spawn hostapd (is it installed?): {e}"))?;

        // Give hostapd a moment to claim the radio and bring up the AP, then
        // make sure it did not exit immediately (e.g. unusable channel).
        std::thread::sleep(std::time::Duration::from_millis(1500));
        if let Ok(Some(status)) = hostapd.try_wait() {
            let _ = std::fs::remove_file(&conf_path);
            let _ = run_ok(Command::new("nmcli").args([
                "device", "set", iface, "managed", "yes",
            ]));
            return Err(format!(
                "hostapd exited during startup ({status}); the radio likely \
                 cannot host an AP on the selected band/channel"
            ));
        }

        // Assign the gateway address so clients have something to route to.
        let _ = run_ok(Command::new("ip").args(["addr", "flush", "dev", iface]));
        run_ok(Command::new("ip").args(["addr", "add", GATEWAY_CIDR, "dev", iface]))
            .map_err(|e| format!("failed to assign {GATEWAY_CIDR} to {iface}: {e}"))?;

        // Start the DHCP server so the phone can lease an address. A missing
        // dnsmasq is non-fatal (the AP is still up) but is logged so the
        // operator knows the phone will not get an IP.
        let dnsmasq = Command::new("dnsmasq")
            .args([
                "--keep-in-foreground",
                "--bind-dynamic",
                "--except-interface=lo",
                &format!("--interface={iface}"),
                &format!("--dhcp-range={DHCP_RANGE}"),
                &format!("--dhcp-option=3,{GATEWAY_IP}"),
                &format!("--dhcp-option=6,{GATEWAY_IP}"),
                "--no-resolv",
            ])
            .spawn()
            .map_err(|e| {
                log::warn!("failed to spawn dnsmasq ({e}); phone will not get a DHCP lease");
                e
            })
            .ok();

        log::info!(
            "hostapd hotspot '{ssid}' up on {iface} (channel {})",
            if channel == 0 { "auto".to_string() } else { channel.to_string() }
        );

        Ok(Self {
            iface: iface.to_string(),
            hostapd,
            dnsmasq,
            conf_path,
        })
    }
}

impl Drop for HostapdHandle {
    fn drop(&mut self) {
        // Stop the DHCP server and the AP.
        if let Some(mut dnsmasq) = self.dnsmasq.take() {
            let _ = dnsmasq.kill();
            let _ = dnsmasq.wait();
        }
        let _ = self.hostapd.kill();
        let _ = self.hostapd.wait();

        // Drop the gateway address and hand the radio back to NetworkManager.
        let _ = run_ok(Command::new("ip").args(["addr", "flush", "dev", &self.iface]));
        let _ = run_ok(Command::new("nmcli").args([
            "device", "set", &self.iface, "managed", "yes",
        ]));

        let _ = std::fs::remove_file(&self.conf_path);
        log::info!("hostapd hotspot on {} torn down", self.iface);
    }
}

/// Render a `hostapd` configuration for a WPA2-PSK access point.
fn hostapd_conf(iface: &str, ssid: &str, psk: &str, channel: i32) -> String {
    // channel 0 means "auto"; pick a common non-DFS 5 GHz channel as the
    // default since Android Auto wireless requires a 5 GHz AP.
    let ch = if channel == 0 { 36 } else { channel };
    let is_5ghz = ch >= 36;
    let hw_mode = if is_5ghz { "a" } else { "g" };

    let mut s = String::new();
    s.push_str(&format!("interface={iface}\n"));
    s.push_str("driver=nl80211\n");
    s.push_str(&format!("ssid={ssid}\n"));
    s.push_str(&format!("hw_mode={hw_mode}\n"));
    s.push_str(&format!("channel={ch}\n"));
    s.push_str("ieee80211n=1\n");
    if is_5ghz {
        s.push_str("ieee80211ac=1\n");
        // Advertise country/DFS info so DFS-capable hardware can legally use
        // radar channels on a real head unit.
        s.push_str("ieee80211d=1\n");
        s.push_str("ieee80211h=1\n");
    }
    s.push_str("wmm_enabled=1\n");
    s.push_str("auth_algs=1\n");
    s.push_str("wpa=2\n");
    s.push_str("wpa_key_mgmt=WPA-PSK\n");
    s.push_str("rsn_pairwise=CCMP\n");
    s.push_str(&format!("wpa_passphrase={psk}\n"));
    s
}

/// Run a command to completion and map a non-zero exit to an error.
fn run_ok(cmd: &mut Command) -> Result<(), String> {
    let output = cmd.output().map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "command failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

/// Write `bytes` to `path` with owner-only (0600) permissions so the PSK in
/// the hostapd config is not world-readable.
fn write_private(path: &PathBuf, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)?;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    file.write_all(bytes)?;
    Ok(())
}
