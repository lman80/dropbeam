//! Small engine hooks for the iOS native shell (plugins/native-ui):
//!
//! - `network_changed`: the shell calls this when the app returns to the foreground or
//!   the path changes (Wi-Fi ↔ cellular), so iroh re-probes its addresses and relay at
//!   once instead of waiting for its own netmon (which iOS suspends with the app).
//! - `lan_self_info` / `lan_peer_found`: iOS blocks raw multicast without Apple's
//!   multicast entitlement, so iroh's mDNS (swarm-discovery) can't see the LAN there.
//!   The shell advertises/browses a Bonjour service (`_dropbeam._udp`) through the
//!   system's mDNSResponder instead and feeds what it finds into an in-memory address
//!   lookup, so a nearby device is dialed directly over the LAN.
//! - `push_unregister_all`: Settings → Erase All Data withdraws the push token.
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, OnceLock};

use iroh::address_lookup::memory::MemoryLookup;
use serde::Serialize;
use tauri::State;

use crate::iroh_net::IrohState;

static LAN_LOOKUP: OnceLock<MemoryLookup> = OnceLock::new();

/// Re-probe the network now (foreground / path change). Harmless if nothing changed.
#[tauri::command]
pub async fn network_changed(iroh: State<'_, Arc<IrohState>>) -> Result<(), String> {
    if let Some(ep) = iroh.get().cloned() {
        ep.network_change().await;
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LanSelf {
    pub endpoint_id: String,
    /// This device's direct LAN socket addresses (private / link-local IPv4, ULA IPv6).
    pub addrs: Vec<String>,
}

/// What the shell advertises in its Bonjour TXT record.
#[tauri::command]
pub fn lan_self_info(iroh: State<'_, Arc<IrohState>>) -> Result<LanSelf, String> {
    let ep = iroh.get().ok_or("The network is still starting.")?;
    let addrs = ep.addr().ip_addrs().copied().filter(|a| is_lan(a.ip()) && a.port() != 0).take(8).map(|a| a.to_string()).collect();
    Ok(LanSelf { endpoint_id: ep.id().to_string(), addrs })
}

/// A device seen over Bonjour: remember its LAN addresses for the next dial.
/// Connections stay authenticated by the endpoint key, so a wrong advertisement can
/// at worst cost a failed path attempt — and only LAN addresses are accepted.
#[tauri::command]
pub fn lan_peer_found(iroh: State<'_, Arc<IrohState>>, endpoint_id: String, addrs: Vec<String>) -> Result<bool, String> {
    let ep = iroh.get().ok_or("The network is still starting.")?;
    let id: iroh::EndpointId = endpoint_id.trim().parse().map_err(|_| "Invalid endpoint id.".to_string())?;
    if id == ep.id() {
        return Ok(false);
    }
    let sockets = lan_sockets(&addrs);
    if sockets.is_empty() {
        return Ok(false);
    }
    let lookup = LAN_LOOKUP.get_or_init(|| {
        let lookup = MemoryLookup::with_provenance("bonjour");
        match ep.address_lookup() {
            Ok(services) => services.add(lookup.clone()),
            Err(e) => log::warn!("bonjour: address lookup unavailable: {e}"),
        }
        lookup
    });
    let mut addr = iroh::EndpointAddr::new(id);
    for socket in sockets {
        addr = addr.with_ip_addr(socket);
    }
    lookup.add_endpoint_info(addr);
    log::info!("bonjour: a nearby device is reachable on the local network");
    Ok(true)
}

/// Erase All Data: withdraw this phone's push token from every Transfer Server.
#[tauri::command]
pub async fn push_unregister_all(
    state: State<'_, Arc<crate::AppState>>,
    iroh: State<'_, Arc<IrohState>>,
) -> Result<usize, String> {
    let config = state.config_dir.clone();
    Ok(crate::mailbox::push::unregister_everywhere(&iroh, &config).await)
}

/// Erase All Data, last engine step: close the network endpoint so nothing new arrives
/// (or gets written for it) while the shell wipes the files and closes the app.
#[tauri::command]
pub async fn network_shutdown(iroh: State<'_, Arc<IrohState>>) -> Result<(), String> {
    if let Some(ep) = iroh.get().cloned() {
        let _ = tokio::time::timeout(std::time::Duration::from_secs(3), ep.close()).await;
    }
    Ok(())
}

fn lan_sockets(addrs: &[String]) -> Vec<SocketAddr> {
    let mut out: Vec<SocketAddr> = addrs
        .iter()
        .filter_map(|a| a.trim().parse::<SocketAddr>().ok())
        .filter(|a| is_lan(a.ip()) && a.port() != 0)
        .collect();
    out.dedup();
    out.truncate(8);
    out
}

/// Addresses that only mean something on the local network.
fn is_lan(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_private() || v4.is_link_local(),
        // Unique local (fc00::/7). Link-local v6 needs an interface scope we can't carry.
        IpAddr::V6(v6) => (v6.segments()[0] & 0xfe00) == 0xfc00,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_lan_addresses_are_accepted() {
        let got = lan_sockets(&[
            "192.168.1.20:52000".into(),
            "10.0.0.4:1".into(),
            "169.254.3.3:9".into(),
            "[fd12::1]:7000".into(),
            "8.8.8.8:53".into(),
            "[2001:db8::1]:80".into(),
            "192.168.1.20:0".into(),
            "nonsense".into(),
        ]);
        let got: Vec<String> = got.iter().map(|a| a.to_string()).collect();
        assert_eq!(got, ["192.168.1.20:52000", "10.0.0.4:1", "169.254.3.3:9", "[fd12::1]:7000"]);
    }

    #[test]
    fn at_most_eight_addresses() {
        let many: Vec<String> = (1..20).map(|i| format!("192.168.0.{i}:4000")).collect();
        assert_eq!(lan_sockets(&many).len(), 8);
    }
}
