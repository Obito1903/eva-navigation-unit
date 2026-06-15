//! Wi-Fi hotspot backend that drives a privileged `eva-hotspot.service`.
//!
//! The default hotspot backend uses NetworkManager over D-Bus
//! ([`crate::nmrs_extensions`]), which inherits NM's limitations: it cannot act
//! as a DFS master and gives little control over the AP's band/channel, and on
//! some radios (e.g. Broadcom FullMAC `brcmfmac`) NM's wpa_supplicant AP mode
//! fails outright while a standalone `hostapd` succeeds.
//!
//! This backend uses `hostapd` for that more capable path **without running
//! eva-ui as root**. The privileged radio work (taking the interface into AP
//! mode, running `hostapd`/`dnsmasq`, assigning the gateway address) lives in a
//! systemd unit, `eva-hotspot.service`, that runs as root. The unprivileged
//! eva-ui process merely asks systemd to start/stop that one unit:
//!
//! * `systemctl start eva-hotspot.service` — bring the AP up.
//! * `systemctl stop eva-hotspot.service`  — tear it down (also done on drop).
//!
//! A bundled polkit rule authorises exactly this for the head-unit user, so no
//! password, sudo, or broad capability grant is needed. The SSID/PSK/channel/
//! country now live in the service's config (`/etc/eva-hotspot/hotspot.env`),
//! since the unprivileged app no longer configures the radio itself. See
//! `deploy/eva-hotspot/` for the unit, helper script and installer.
//!
//! If the unit is not installed, not authorised, or fails to bring up the AP,
//! [`HostapdHandle::start`] returns an error and the caller falls back to
//! USB-only Android Auto instead of crashing.

use std::process::Command;

/// The systemd unit that owns the privileged hotspot.
const SERVICE: &str = "eva-hotspot.service";

/// A running hotspot owned by `eva-hotspot.service`. Dropping it stops the unit.
pub struct HostapdHandle;

impl HostapdHandle {
    /// Start the privileged hotspot unit and confirm the AP came up.
    ///
    /// Returns an error (so the caller can fall back to USB-only) if the unit
    /// cannot be started — e.g. it is not installed, the polkit rule is missing
    /// so authorisation is denied, or the radio cannot host an AP on the
    /// configured band/channel and `hostapd` exits during startup.
    pub fn start() -> Result<Self, String> {
        // Ask systemd (authorised by the bundled polkit rule) to start the unit.
        run_ok(Command::new("systemctl").args(["start", SERVICE])).map_err(|e| {
            format!(
                "failed to start {SERVICE} (is it installed and the polkit rule \
                 in place? see deploy/eva-hotspot/): {e}"
            )
        })?;

        // `Type=simple` reports "active" as soon as hostapd is exec'd, so give
        // it a moment to claim the radio and then confirm it did not fail (e.g.
        // an unusable channel makes hostapd exit, flipping the unit to failed).
        std::thread::sleep(std::time::Duration::from_millis(1500));
        if !is_active() {
            let detail = unit_failure_detail();
            // Best-effort cleanup so a failed unit does not linger.
            let _ = run_ok(Command::new("systemctl").args(["stop", SERVICE]));
            return Err(format!(
                "{SERVICE} did not come up; the radio likely cannot host an AP \
                 on the configured band/channel{detail}"
            ));
        }

        log::info!("hostapd hotspot up via {SERVICE}");
        Ok(Self)
    }
}

impl Drop for HostapdHandle {
    fn drop(&mut self) {
        let _ = run_ok(Command::new("systemctl").args(["stop", SERVICE]));
        log::info!("hostapd hotspot ({SERVICE}) stopped");
    }
}

/// Whether the hotspot unit is currently active.
fn is_active() -> bool {
    Command::new("systemctl")
        .args(["is-active", "--quiet", SERVICE])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Best-effort tail of the unit's status for context when startup fails.
/// Returns a string beginning with `: ` so it can be appended to an error, or
/// an empty string when nothing useful is available.
fn unit_failure_detail() -> String {
    let Ok(output) = Command::new("systemctl")
        .args(["status", "--no-pager", "-n", "8", SERVICE])
        .output()
    else {
        return String::new();
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let tail = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .rev()
        .take(5)
        .collect::<Vec<_>>();
    if tail.is_empty() {
        return String::new();
    }
    let joined = tail.into_iter().rev().collect::<Vec<_>>().join(" | ");
    let truncated: String = joined.chars().take(400).collect();
    format!(": {truncated}")
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
