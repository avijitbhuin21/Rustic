//! mDNS advertisement + browsing for local-network sync.

use std::collections::HashMap;

use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};

use super::{Discovered, Identity, LanState, SERVICE_TYPE};

/// Advertise this device on `port` and keep `lan.discovered` up to date with
/// other Rustic instances. Returns the daemon; dropping/shutting it down stops both.
pub fn start(lan: &LanState, id: &Identity, port: u16) -> Result<ServiceDaemon, String> {
    let daemon = ServiceDaemon::new().map_err(|e| format!("mDNS unavailable: {e}"))?;
    let host = format!("{}.local.", id.device_id);
    let mut props: HashMap<String, String> = HashMap::new();
    props.insert("id".into(), id.device_id.clone());
    props.insert("name".into(), id.device_name.clone());
    props.insert("fp".into(), id.fingerprint.clone());
    props.insert("v".into(), "1".into());
    props.insert("ver".into(), super::app_version().into());
    let info = ServiceInfo::new(SERVICE_TYPE, &id.device_id, &host, "", port, props)
        .map_err(|e| format!("mDNS service info: {e}"))?
        .enable_addr_auto();
    daemon
        .register(info)
        .map_err(|e| format!("mDNS register: {e}"))?;

    let rx = daemon
        .browse(SERVICE_TYPE)
        .map_err(|e| format!("mDNS browse: {e}"))?;
    let lan = lan.clone();
    let me = id.device_id.clone();
    std::thread::spawn(move || {
        while let Ok(event) = rx.recv() {
            match event {
                ServiceEvent::ServiceResolved(info) => {
                    let Some(device_id) = info.get_property_val_str("id").map(str::to_string)
                    else {
                        continue;
                    };
                    if device_id == me {
                        continue;
                    }
                    let Some(ip) = info.get_addresses_v4().into_iter().next().copied() else {
                        continue;
                    };
                    let entry = Discovered {
                        device_id: device_id.clone(),
                        name: info
                            .get_property_val_str("name")
                            .unwrap_or("Rustic")
                            .to_string(),
                        fingerprint: info.get_property_val_str("fp").unwrap_or("").to_string(),
                        addr: format!("{}:{}", ip, info.get_port()),
                        // Builds before the version check didn't advertise one.
                        version: Some(info.get_property_val_str("ver").unwrap_or("").to_string()),
                        fullname: info.get_fullname().to_string(),
                    };
                    lan.lock().discovered.insert(device_id, entry);
                }
                ServiceEvent::ServiceRemoved(_, fullname) => {
                    lan.lock().discovered.retain(|_, d| d.fullname != fullname);
                }
                ServiceEvent::SearchStopped(_) => break,
                _ => {}
            }
        }
    });
    Ok(daemon)
}
