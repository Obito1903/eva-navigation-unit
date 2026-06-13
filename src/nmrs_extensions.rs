//! Hotspot management via NetworkManager D-Bus (ported from the android-auto example).

use std::collections::HashMap;
use zbus::{Connection, proxy};
use zvariant::{OwnedObjectPath, OwnedValue};

type NmSettings = HashMap<String, HashMap<String, OwnedValue>>;

#[proxy(
    interface = "org.freedesktop.NetworkManager",
    default_service = "org.freedesktop.NetworkManager",
    default_path = "/org/freedesktop/NetworkManager"
)]
trait NmProxy {
    fn add_and_activate_connection(
        &self,
        connection: &NmSettings,
        device: &OwnedObjectPath,
        specific_object: &OwnedObjectPath,
    ) -> zbus::Result<(OwnedObjectPath, OwnedObjectPath)>;
}

fn to_owned_settings(
    input: HashMap<&str, HashMap<&str, zvariant::Value<'_>>>,
) -> HashMap<String, HashMap<String, OwnedValue>> {
    input
        .into_iter()
        .map(|(section, props)| {
            let owned_props = props
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.try_to_owned().unwrap()))
                .collect();
            (section.to_string(), owned_props)
        })
        .collect()
}

pub async fn start_hotspot(ssid: String, psk: String, wifi_dev_path: &str) -> Result<(), String> {
    let hotspot = nmrs::builders::WifiConnectionBuilder::new(&ssid)
        .wpa_psk(&psk)
        .autoconnect(false)
        .mode(nmrs::builders::WifiMode::Ap)
        // Android Auto wireless only works over a 5 GHz access point; a 2.4 GHz
        // AP lets the phone associate but it abandons the link during DHCP
        // provisioning, so projection never starts. Force the 5 GHz band.
        //
        // The channel is deliberately left unset so wpa_supplicant's automatic
        // channel selection picks a 5 GHz channel that is valid for the host's
        // regulatory domain. Pinning a fixed channel (e.g. UNII-1 ch 36) breaks
        // in regions where that channel is not permitted — e.g. under the FR
        // domain only DFS channels are allowed, so a hard-coded ch 36 request
        // is dropped and NM falls all the way back to 2.4 GHz.
        .band(nmrs::builders::WifiBand::A)
        // An access point must use NetworkManager's "shared" IPv4 method so NM
        // assigns the 10.42.0.1/24 gateway address and runs a DHCP/DNS server
        // (dnsmasq) for clients. Without this NM defaults to "auto", runs a
        // DHCP *client* on the AP interface, finds no server and fails with
        // "IP configuration could not be reserved" — so the hotspot never
        // comes up and the phone cannot reach the Android Auto TCP socket.
        .ipv4_shared()
        .build();
    build_hotspot(wifi_dev_path, hotspot).await
}

async fn build_hotspot(
    wifi_hw: &str,
    settings: HashMap<&str, HashMap<&str, zvariant::Value<'_>>>,
) -> Result<(), String> {
    let settings = to_owned_settings(settings);
    let dbus = Connection::system().await.map_err(|e| e.to_string())?;
    let wifi_device = OwnedObjectPath::try_from(wifi_hw).map_err(|e| e.to_string())?;
    let any = OwnedObjectPath::try_from("/").unwrap();
    let nm = NmProxyProxy::new(&dbus).await.map_err(|e| e.to_string())?;
    let (conn_path, _active) = nm
        .add_and_activate_connection(&settings, &wifi_device, &any)
        .await
        .map_err(|e| e.to_string())?;
    log::info!("Hotspot connection path: {conn_path}");
    Ok(())
}
