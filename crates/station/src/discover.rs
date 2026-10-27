//! Finding cameo-pis on the network (cameo-pi spec §11): each Pi advertises `_cameo-pi._tcp`
//! over Bonjour. Finding one only says where it is; pairing still asks the Pi for access.

use std::time::{Duration, Instant};

use mdns_sd::{ServiceDaemon, ServiceEvent};

const SERVICE: &str = "_cameo-pi._tcp.local.";

/// A Pi that answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    /// What it calls itself: "cameo-pi on cameopi".
    pub name: String,
    /// The address to pair with: its `.local` name, with the port when it isn't 443.
    pub address: String,
    /// Its IPv4 addresses, for showing.
    pub ips: Vec<String>,
}

/// Listens for `wait`, then returns every Pi that answered, by name.
pub fn discover(wait: Duration) -> Result<Vec<Found>, String> {
    let daemon = ServiceDaemon::new().map_err(|e| format!("Bonjour isn't available: {e}"))?;
    let events = daemon
        .browse(SERVICE)
        .map_err(|e| format!("Bonjour isn't available: {e}"))?;
    let deadline = Instant::now() + wait;
    let mut found: Vec<Found> = Vec::new();
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        let Ok(event) = events.recv_timeout(left) else {
            break;
        };
        if let ServiceEvent::ServiceResolved(service) = event {
            let host = service.get_hostname().trim_end_matches('.');
            let port = service.get_port();
            let address = if port == 443 {
                host.to_owned()
            } else {
                format!("{host}:{port}")
            };
            let mut ips: Vec<String> = service
                .get_addresses_v4()
                .iter()
                .map(ToString::to_string)
                .collect();
            ips.sort();
            let name = service
                .get_fullname()
                .strip_suffix(SERVICE)
                .unwrap_or(service.get_fullname())
                .trim_end_matches('.')
                .replace("\\032", " ");
            found.retain(|f| f.address != address);
            found.push(Found { name, address, ips });
        }
    }
    let _ = daemon.shutdown();
    found.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(found)
}
