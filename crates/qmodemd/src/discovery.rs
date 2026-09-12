// SPDX-License-Identifier: GPL-3.0-only
// Device rules and model detection derived from FUjr/QModem modem_scand.c.
use crate::{
    at::{PortPool, Sequence, Step},
    config::{Bus, Modem},
};
use anyhow::{Context, Result, ensure};
use futures_util::{StreamExt, stream};
use serde::Serialize;
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Debug, Clone, Serialize)]
pub struct Device {
    pub id: String,
    pub bus: String,
    pub sysfs_path: String,
    pub vendor_id: String,
    pub product_id: String,
    pub serial: Option<String>,
    pub at_candidates: Vec<String>,
    pub voice_pcm_port: Option<String>,
    pub network_interfaces: Vec<String>,
    pub control_ports: Vec<String>,
    pub needs_option_binding: bool,
    pub valid_at_ports: Vec<String>,
    pub modem: Option<Modem>,
    pub errors: Vec<String>,
}
fn read(path: impl AsRef<Path>) -> String {
    fs::read_to_string(path)
        .unwrap_or_default()
        .trim()
        .to_owned()
}
fn entries(path: &Path) -> Vec<PathBuf> {
    let mut result: Vec<_> = fs::read_dir(path)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok().map(|v| v.path()))
        .collect();
    result.sort();
    result
}
fn name(path: &Path) -> &str {
    path.file_name().and_then(|v| v.to_str()).unwrap_or("")
}
fn hex(path: &Path) -> String {
    read(path).trim_start_matches("0x").to_ascii_lowercase()
}
fn catalogue() -> Value {
    serde_json::from_str(include_str!("../../../data/supported-models.json"))
        .expect("bundled catalogue")
}
fn rules() -> Value {
    serde_json::from_str(include_str!("../../../data/modem_port_rule.json")).expect("bundled rules")
}
fn collect(path: &Path, depth: usize, f: &mut impl FnMut(&Path)) {
    if depth == 0 {
        return;
    }
    for child in entries(path) {
        f(&child);
        if child.symlink_metadata().is_ok_and(|m| m.is_dir()) {
            collect(&child, depth - 1, f);
        }
    }
}
fn ports_under(path: &Path) -> Vec<String> {
    let mut ports = Vec::new();
    collect(path, 3, &mut |p| {
        let n = name(p);
        if n.starts_with("ttyUSB") || n.starts_with("ttyACM") {
            ports.push(format!("/dev/{n}"));
        }
    });
    ports.sort();
    ports.dedup();
    ports
}
fn identity(bus: &str, slot: &str) -> String {
    format!(
        "{bus}-{}",
        slot.chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect::<String>()
    )
}
/// Read-only inventory. No driver binding and no AT traffic on this path.
pub fn scan(sys: &Path) -> Result<Vec<Device>> {
    ensure!(
        sys.join("bus").is_dir(),
        "sysfs bus directory is unavailable"
    );
    let rules = rules();
    let mut result = Vec::new();
    for bus in ["usb", "pci"] {
        for slot in entries(&sys.join(format!("bus/{bus}/devices"))) {
            let usb = bus == "usb";
            let vid = hex(&slot.join(if usb { "idVendor" } else { "vendor" }));
            let pid = hex(&slot.join(if usb { "idProduct" } else { "device" }));
            if usb {
                if vid != "2c7c" && !(vid == "3466" && pid == "3301") {
                    continue;
                }
            } else if !["1eac", "2c7c", "17cb"].contains(&vid.as_str()) {
                continue;
            }
            let mut device = Device {
                id: identity(if usb { "usb" } else { "pcie" }, name(&slot)),
                bus: if usb { "usb" } else { "pcie" }.into(),
                sysfs_path: slot.to_string_lossy().into_owned(),
                vendor_id: vid.clone(),
                product_id: pid.clone(),
                serial: Some(read(slot.join("serial"))).filter(|s| !s.is_empty()),
                at_candidates: vec![],
                voice_pcm_port: None,
                network_interfaces: vec![],
                control_ports: vec![],
                needs_option_binding: false,
                valid_at_ports: vec![],
                modem: None,
                errors: vec![],
            };
            if usb {
                let rule = &rules["modem_port_rule"]["usb"][format!("{vid}:{pid}")];
                device.needs_option_binding = rule["option_driver"].as_u64() == Some(1);
                for interface in entries(&slot) {
                    if !name(&interface).starts_with(&format!("{}:", name(&slot))) {
                        continue;
                    }
                    let suffix = name(&interface).rsplit(':').next().unwrap_or("");
                    let driver = fs::read_link(interface.join("driver")).unwrap_or_default();
                    let driver = name(&driver);
                    if [
                        "option",
                        "cdc_acm",
                        "qcserial",
                        "usbserial_generic",
                        "usbserial",
                    ]
                    .contains(&driver)
                    {
                        let ports = ports_under(&interface);
                        if rule["voice_pcm_interface"].as_str() == Some(suffix) {
                            if ports.len() == 1 {
                                device.voice_pcm_port = ports.first().cloned();
                            }
                            continue;
                        }
                        if rule["include"].as_array().is_some_and(|a| {
                            !a.is_empty() && !a.iter().any(|v| v.as_str() == Some(suffix))
                        }) {
                            continue;
                        }
                        device.at_candidates.extend(ports);
                    } else if driver.starts_with("qmi_wwan")
                        || driver.contains("cdc_ncm")
                        || ["cdc_mbim", "cdc_ether", "rndis_host"].contains(&driver)
                    {
                        device.network_interfaces.extend(
                            entries(&interface.join("net"))
                                .iter()
                                .map(|p| name(p).into()),
                        );
                        device.control_ports.extend(
                            entries(&interface.join("usbmisc"))
                                .iter()
                                .map(|p| format!("/dev/{}", name(p))),
                        );
                    }
                }
            } else {
                // Walk only actual children: sysfs driver/subsystem links must never be followed.
                // Covers mhi_uci_q, upstream mhi_wwan_ctrl and t7xx-style WWAN nodes.
                collect(&slot, 7, &mut |p| {
                    let n = name(p);
                    if n.starts_with("mhi_DUN") || (n.starts_with("wwan") && n.contains("at")) {
                        device.at_candidates.push(format!("/dev/{n}"));
                    }
                    if n.starts_with("mhi_QMI")
                        || (n.starts_with("wwan") && (n.contains("qmi") || n.contains("mbim")))
                    {
                        device.control_ports.push(format!("/dev/{n}"));
                    }
                    if name(p.parent().unwrap_or(p)) == "net" {
                        device.network_interfaces.push(n.into());
                    }
                });
                if device.at_candidates.is_empty() && device.network_interfaces.is_empty() {
                    continue;
                }
            }
            for list in [
                &mut device.at_candidates,
                &mut device.network_interfaces,
                &mut device.control_ports,
            ] {
                list.sort();
                list.dedup();
            }
            result.push(device);
        }
    }
    Ok(result)
}
fn model_from_reply(reply: &str, bus: &str) -> Option<(String, Value)> {
    let catalog = catalogue();
    let profiles = catalog["modem_support"][bus].as_object()?;
    for line in reply.lines().map(str::trim) {
        if line.is_empty() || line == "OK" || line.starts_with("AT") {
            continue;
        }
        let mut candidate = line
            .strip_prefix("+CGMM:")
            .unwrap_or(line)
            .trim()
            .trim_matches('"')
            .to_ascii_lowercase();
        for alias in ["em120k", "rm500u-ea", "rg200u-cn"] {
            if candidate.contains(alias) {
                candidate = alias.into();
                break;
            }
        }
        if let Some(profile) = profiles.get(&candidate) {
            return Some((candidate, profile.clone()));
        }
    }
    None
}
fn modem_config(device: &Device, model: String, profile: Value) -> Option<Modem> {
    Some(Modem {
        id: device.id.clone(),
        name: model.clone(),
        enabled: true,
        manufacturer: profile["manufacturer"].as_str()?.into(),
        model,
        platform: profile["platform"].as_str()?.into(),
        at_port: device.valid_at_ports.first()?.clone(),
        sms_at_port: device.valid_at_ports.get(1).cloned(),
        interface: device.network_interfaces.first().cloned(),
        bus: if device.bus == "usb" {
            Bus::Usb
        } else {
            Bus::Pcie
        },
        pdp_index: profile["pdp_index"]
            .as_str()
            .and_then(|s| s.parse().ok())
            .unwrap_or(1),
        apn: String::new(),
        bands: Default::default(),
        sms: Default::default(),
        network: Default::default(),
    })
}
/// Native, bounded identification. Never resets an existing queue or transmits a write command.
pub async fn probe(mut device: Device, pool: &PortPool) -> Device {
    let mut results = stream::iter(device.at_candidates.iter().cloned().map(|path| async move {
        let result = async {
            let port = pool.get(&path).await?;
            let replies = port
                .run_named(
                    Box::new(Sequence::new(
                        vec![Step::command("ATI", Duration::from_secs(2))?],
                        false,
                    )),
                    Some(device_id(&path)),
                    "discovery",
                )
                .await?;
            ensure!(
                replies
                    .iter()
                    .any(|r| r.response.contains("OK") || r.response.contains("ATI")),
                "port did not answer ATI"
            );
            Ok::<_, anyhow::Error>(())
        }
        .await;
        (path, result)
    }))
    .buffer_unordered(4)
    .collect::<Vec<_>>()
    .await;
    results.sort_by(|a, b| a.0.cmp(&b.0));
    for (path, result) in results {
        match result {
            Ok(()) => device.valid_at_ports.push(path),
            Err(_) => device.errors.push(format!("{path}: identification failed")),
        }
    }
    'ports: for path in &device.valid_at_ports {
        for command in ["AT+CGMM", "AT+CGMM?", "AT+GMM"] {
            let Ok(port) = pool.get(path).await else {
                continue;
            };
            let step = Step::command(command, Duration::from_secs(5)).expect("constant command");
            let Ok(replies) = port
                .run_named(
                    Box::new(Sequence::new(vec![step], false)),
                    Some(device.id.clone()),
                    "discovery_model",
                )
                .await
            else {
                continue;
            };
            if let Some((model, profile)) = replies
                .iter()
                .find_map(|r| model_from_reply(&r.response, &device.bus))
            {
                device.modem = modem_config(&device, model, profile);
                break 'ports;
            }
        }
    }
    if device.modem.is_none() {
        let catalog = catalogue();
        if let Some(profiles) = catalog["modem_support"][&device.bus].as_object() {
            let id = format!("{}:{}", device.vendor_id, device.product_id);
            if let Some((model, profile)) =
                profiles.iter().find(|(_, p)| p["id"].as_str() == Some(&id))
            {
                device.modem = modem_config(&device, model.clone(), profile.clone());
            }
        }
    }
    device
}
fn device_id(path: &str) -> String {
    format!("probe-{}", path.rsplit('/').next().unwrap_or("port"))
}
/// Limited to IDs whose upstream rules require option driver registration.
pub fn bind_option(device: &Device, sys: &Path) -> Result<()> {
    ensure!(
        device.needs_option_binding,
        "this device has no option binding rule"
    );
    let current = scan(sys)?
        .into_iter()
        .find(|d| d.id == device.id)
        .context("device disappeared")?;
    ensure!(
        current.vendor_id == device.vendor_id && current.product_id == device.product_id,
        "device identity changed"
    );
    fs::write(
        sys.join("bus/usb-serial/drivers/option1/new_id"),
        format!("{} {}\n", device.vendor_id, device.product_id),
    )
    .context("register option driver")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    fn file(root: &Path, path: &str, value: &str) {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, value).unwrap();
    }
    fn serial(root: &Path, slot: &str, interface: &str, port: &str) {
        let path = root.join(format!("bus/usb/devices/{slot}/{slot}:{interface}"));
        fs::create_dir_all(path.join(port)).unwrap();
        symlink("/sys/bus/usb/drivers/option", path.join("driver")).unwrap();
    }
    #[test]
    fn usb_rules_exclude_pcm_and_unrelated_vendors() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        for (slot, vid, pid) in [
            ("1-1", "2c7c", "0801"),
            ("1-2", "3466", "3301"),
            ("1-3", "2cb7", "0104"),
            ("1-4", "3466", "ffff"),
        ] {
            file(root, &format!("bus/usb/devices/{slot}/idVendor"), vid);
            file(root, &format!("bus/usb/devices/{slot}/idProduct"), pid);
        }
        for (interface, port) in [
            ("1.0", "ttyUSB0"),
            ("1.1", "ttyUSB1"),
            ("1.2", "ttyUSB2"),
            ("1.3", "ttyUSB3"),
        ] {
            serial(root, "1-1", interface, port);
        }
        let devices = scan(root).unwrap();
        assert_eq!(devices.len(), 2);
        assert_eq!(
            devices[0].at_candidates,
            vec!["/dev/ttyUSB2", "/dev/ttyUSB3"]
        );
        assert_eq!(devices[0].voice_pcm_port.as_deref(), Some("/dev/ttyUSB1"));
        assert!(devices[1].needs_option_binding);
        assert!(
            devices
                .iter()
                .all(|d| d.modem.is_none() && d.valid_at_ports.is_empty())
        );
    }
    #[test]
    fn pcie_discovers_nonzero_wwan_and_mhi_without_following_symlinks() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        file(root, "bus/pci/devices/0000:03:00.0/vendor", "0x1eac");
        file(root, "bus/pci/devices/0000:03:00.0/device", "0x1001");
        let base = root.join("bus/pci/devices/0000:03:00.0");
        for child in [
            "mhi3/wwan/wwan7/wwan7at0",
            "03.00.0_DUN/mhi_uci_q/mhi_DUN3",
            "mhi3/net/wwan7",
            "mhi3/wwan/wwan7/wwan7qmi0",
        ] {
            fs::create_dir_all(base.join(child)).unwrap();
        }
        symlink(&base, base.join("subsystem")).unwrap();
        let devices = scan(root).unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(
            devices[0].at_candidates,
            vec!["/dev/mhi_DUN3", "/dev/wwan7at0"]
        );
        assert_eq!(devices[0].network_interfaces, vec!["wwan7"]);
        assert_eq!(devices[0].control_ports, vec!["/dev/wwan7qmi0"]);
    }
    #[test]
    fn model_detection_preserves_upstream_aliases_and_scope() {
        assert_eq!(
            model_from_reply("AT+CGMM\r\n+CGMM: \"RM520N-GL\"\r\nOK", "usb")
                .unwrap()
                .0,
            "rm520n-gl"
        );
        assert_eq!(
            model_from_reply("RM500U-EA Revision\r\nOK", "usb")
                .unwrap()
                .0,
            "rm500u-ea"
        );
        assert!(model_from_reply("FM350-GL\r\nOK", "pcie").is_none());
        assert!(model_from_reply("MT5700M-CN\r\nOK", "usb").is_some());
    }
    #[test]
    fn binding_rechecks_identity() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        file(root, "bus/usb/devices/1-2/idVendor", "3466");
        file(root, "bus/usb/devices/1-2/idProduct", "3301");
        file(root, "bus/usb-serial/drivers/option1/new_id", "");
        let device = scan(root).unwrap().remove(0);
        bind_option(&device, root).unwrap();
        assert_eq!(
            read(root.join("bus/usb-serial/drivers/option1/new_id")),
            "3466 3301"
        );
        file(root, "bus/usb/devices/1-2/idProduct", "ffff");
        assert!(bind_option(&device, root).is_err());
    }
}
