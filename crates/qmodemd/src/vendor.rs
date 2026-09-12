// SPDX-License-Identifier: GPL-3.0-only
// AT behavior ported from FUjr/QModem vendor/quectel.sh and vendor/huawei.sh.
// Copyright (C) 2023 Siriling <siriling@qq.com>
// Copyright (C) 2025 Fujr <fjrcn@outlook.com>
// Copyright (C) 2025 coolsnowwolf <coolsnowwolf@gmail.com>
// Rust adaptation Copyright (C) 2026 tcpqueue
//! AT compatibility is separate from transport and HTTP. Do not infer a command
//! family from USB vendor name alone: MT5700 is tagged "huawei" in upstream data.
pub mod bands;
pub mod cells;
mod sim;
mod transaction;
mod usage;
pub use sim::Runtime;
pub use transaction::{finish, local, plan};

use crate::{
    at::{Reply, Step},
    config::Modem,
};
use anyhow::{Result, bail, ensure};
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Quectel,
    TdtechMt5700,
}
pub fn family(device: &Modem) -> Result<Family> {
    match device.manufacturer.to_ascii_lowercase().as_str() {
        "quectel" => Ok(Family::Quectel),
        "tdtech"
            if ["mt5700", "mt5700m", "mt5700m-cn"]
                .contains(&device.model.to_ascii_lowercase().as_str()) =>
        {
            ensure!(
                device.platform == "hisilicon",
                "TD Tech MT5700 requires the hisilicon command profile"
            );
            Ok(Family::TdtechMt5700)
        }
        _ => bail!(
            "supported modem scope: Quectel, or TD Tech MT5700 (manufacturer=tdtech, model=mt5700m-cn, platform=hisilicon)"
        ),
    }
}
#[derive(Debug, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    GetImei,
    GetMode,
    SetMode {
        mode: String,
    },
    GetNetworkPrefer,
    SetNetworkPrefer {
        networks: Vec<String>,
    },
    #[serde(rename = "get_5g_lan")]
    Get5gLan,
    #[serde(rename = "set_5g_lan")]
    Set5gLan {
        enabled: bool,
    },
    GetSimSlot,
    GetSimCapabilities,
    SetSimSlot {
        slot: u8,
    },
    GetBandLock,
    SetBandLock {
        band_class: bands::Class,
        bands: Vec<u16>,
    },
    SetImei {
        imei: String,
    },
    SoftReboot,
    GetNeighborcell,
    SetCellLock {
        lock: cells::Lock,
    },
    UnlockCell,
    GetUsageStats,
    WriteUsageStats,
    ClearUsageStats,
}
impl Operation {
    pub fn name(&self) -> &'static str {
        match self {
            Self::GetNeighborcell => "get_neighborcell",
            Self::SetCellLock { .. } => "set_cell_lock",
            Self::UnlockCell => "unlock_cell",
            Self::GetUsageStats => "get_usage_stats",
            Self::WriteUsageStats => "write_usage_stats",
            Self::ClearUsageStats => "clear_usage_stats",
            Self::GetImei => "get_imei",
            Self::SetImei { .. } => "set_imei",
            Self::GetMode => "get_mode",
            Self::SetMode { .. } => "set_mode",
            Self::GetNetworkPrefer => "get_network_prefer",
            Self::SetNetworkPrefer { .. } => "set_network_prefer",
            Self::Get5gLan => "get_5g_lan",
            Self::Set5gLan { .. } => "set_5g_lan",
            Self::GetSimSlot => "get_sim_slot",
            Self::GetSimCapabilities => "get_sim_capabilities",
            Self::SetSimSlot { .. } => "set_sim_slot",
            Self::GetBandLock => "get_band_lock",
            Self::SetBandLock { .. } => "set_band_lock",
            Self::SoftReboot => "soft_reboot",
        }
    }
}
fn selected(networks: &[String], network: &str) -> bool {
    networks.iter().any(|n| n == network)
}

pub fn prepare(device: &Modem, operation: &Operation) -> Result<Step> {
    let family = family(device)?;
    if let Operation::SetMode { mode } = operation {
        let allowed: &[&str] = match family {
            Family::Quectel => &["qmi", "ecm", "mbim", "rndis", "ncm"],
            Family::TdtechMt5700 => &["ecm", "ncm"],
        };
        ensure!(
            allowed.contains(&mode.as_str()),
            "mode is not valid for this command family"
        );
    }
    let command = match operation {
        Operation::GetNeighborcell
        | Operation::SetCellLock { .. }
        | Operation::UnlockCell
        | Operation::GetUsageStats
        | Operation::WriteUsageStats
        | Operation::ClearUsageStats => bail!("operation requires a transaction"),
        Operation::GetImei => "AT+CGSN".into(),
        Operation::SoftReboot => "AT+CFUN=1,1".into(),
        Operation::GetMode => match family {
            Family::Quectel => "AT+QCFG=\"usbnet\"",
            Family::TdtechMt5700 => "AT^SETMODE?",
        }
        .into(),
        Operation::SetMode { mode } => match family {
            Family::Quectel => {
                // These fallback values deliberately match upstream set_mode's
                // platform switch, including its hisilicon fallback to usbnet=0.
                let n =
                    if ["qualcomm", "unisoc", "lte12", "lte"].contains(&device.platform.as_str()) {
                        match mode.as_str() {
                            "qmi" => 0,
                            "ecm" => 1,
                            "mbim" => 2,
                            "rndis" => 3,
                            "ncm" => 5,
                            _ => 0,
                        }
                    } else {
                        0
                    };
                format!("AT+QCFG=\"usbnet\",{n}")
            }
            Family::TdtechMt5700 => format!("AT^SETMODE={}", if mode == "ncm" { 4 } else { 0 }),
        },
        Operation::GetNetworkPrefer => match family {
            Family::Quectel if device.platform == "lte" => "AT+QCFG=\"nwscanmode\"",
            Family::Quectel => "AT+QNWPREFCFG=\"mode_pref\"",
            Family::TdtechMt5700 => "AT^SYSCFGEX?",
        }
        .into(),
        Operation::SetNetworkPrefer { networks } => {
            ensure!(
                networks
                    .iter()
                    .all(|n| ["3G", "4G", "5G"].contains(&n.as_str())),
                "network preference must use 3G, 4G or 5G"
            );
            ensure!(networks.len() <= 3, "too many network preferences");
            for (index, n) in networks.iter().enumerate() {
                ensure!(
                    !networks[..index].contains(n),
                    "duplicate network preference"
                );
            }
            let g3 = selected(networks, "3G");
            let g4 = selected(networks, "4G");
            let g5 = selected(networks, "5G");
            match family {
                Family::Quectel if device.platform == "lte" => {
                    ensure!(
                        !g5 && !networks.is_empty(),
                        "LTE profile requires 3G and/or 4G"
                    );
                    format!(
                        "AT+QCFG=\"nwscanmode\",{}",
                        if networks.len() == 1 && g4 { 3 } else { 0 }
                    )
                }
                Family::Quectel => {
                    let value = match (g3, g4, g5) {
                        (true, false, false) => "WCDMA",
                        (false, true, false) => "LTE",
                        (false, false, true) => "NR5G",
                        (true, true, false) => "WCDMA:LTE",
                        (true, false, true) => "WCDMA:NR5G",
                        (false, true, true) => "LTE:NR5G",
                        _ => "AUTO",
                    };
                    format!("AT+QNWPREFCFG=\"mode_pref\",{value}")
                }
                Family::TdtechMt5700 => {
                    // Upstream's pair mappings are counterintuitive but are the
                    // compatibility baseline, not silently replaced by AT-manual guesses.
                    let value = match (g3, g4, g5) {
                        (true, false, false) | (true, true, false) => "02",
                        (false, true, false) | (false, true, true) => "03",
                        (false, false, true) | (true, false, true) => "08",
                        (true, true, true) => "080302",
                        _ => "00",
                    };
                    format!("AT^SYSCFGEX=\"{value}\",40000000,1,2,40000000,,")
                }
            }
        }
        Operation::Get5gLan => {
            ensure!(
                family == Family::Quectel,
                "5G LAN is not supported by the MT5700 profile"
            );
            "AT+QCFG=\"5glan\"".into()
        }
        Operation::Set5gLan { enabled } => {
            ensure!(
                family == Family::Quectel,
                "5G LAN is not supported by the MT5700 profile"
            );
            format!("AT+QCFG=\"5glan\",1,{}", u8::from(*enabled))
        }
        Operation::GetSimSlot => {
            ensure!(
                family == Family::Quectel,
                "MT5700 slot is maintained as software state by upstream, not read from an AT query; state migration is pending"
            );
            "AT+QUIMSLOT?".into()
        }
        _ => bail!("operation requires a vendor transaction"),
    };
    Step::command(&command, Duration::from_secs(10))
}

pub fn interpret(device: &Modem, operation: &Operation, reply: &Reply) -> Result<Value> {
    if !reply.modem_success {
        return Ok(json!({"success":false,"terminal":reply.terminal,"response":reply.response}));
    }
    let raw = &reply.response;
    let data = match operation {
        Operation::GetImei => {
            let imei = raw
                .split(|c: char| !c.is_ascii_digit())
                .find(|s| s.len() == 15)
                .ok_or_else(|| anyhow::anyhow!("IMEI response has no 15-digit value"))?;
            json!({"imei":imei})
        }
        Operation::GetMode => {
            let family = family(device)?;
            let value = if family == Family::Quectel {
                raw.lines()
                    .find_map(|l| {
                        l.strip_prefix("+QCFG:")
                            .and_then(|r| r.split_once(',').map(|(_, v)| v.trim()))
                    })
                    .ok_or_else(|| anyhow::anyhow!("missing +QCFG mode response"))?
            } else {
                raw.lines()
                    .map(str::trim)
                    .find(|l| !l.is_empty() && l.bytes().all(|b| b.is_ascii_digit()))
                    .ok_or_else(|| anyhow::anyhow!("missing numeric SETMODE response"))?
            };
            let mode = match family {
                Family::TdtechMt5700 => match value {
                    "0" | "2" => "ecm",
                    "1" | "3" | "4" | "5" => "ncm",
                    "6" => "rndis",
                    "7" => "mbim",
                    "8" => "ppp",
                    _ => "rndis",
                },
                Family::Quectel if device.platform == "hisilicon" => match value {
                    "1" => "ecm",
                    "3" => "rndis",
                    _ => "ncm",
                },
                Family::Quectel
                    if ["qualcomm", "lte", "lte12"].contains(&device.platform.as_str()) =>
                {
                    match value {
                        "0" => "qmi",
                        "1" => "ecm",
                        "2" => "mbim",
                        "3" => "rndis",
                        "5" => "ncm",
                        _ => value,
                    }
                }
                Family::Quectel if device.platform == "unisoc" => match value {
                    "1" => "ecm",
                    "2" => "mbim",
                    "3" => "rndis",
                    "5" => "ncm",
                    _ => value,
                },
                Family::Quectel => value,
            };
            json!({"mode":mode})
        }
        Operation::GetNetworkPrefer => {
            let family = family(device)?;
            let (g3, g4, g5) = match family {
                Family::Quectel if device.platform == "lte" => {
                    let v = raw
                        .lines()
                        .find_map(|l| {
                            l.strip_prefix("+QCFG:")
                                .and_then(|v| v.split_once(',').map(|(_, v)| v.trim()))
                        })
                        .ok_or_else(|| anyhow::anyhow!("missing nwscanmode response"))?;
                    (v == "0", v == "0" || v == "3", false)
                }
                Family::Quectel => {
                    let v = raw
                        .lines()
                        .find_map(|l| l.strip_prefix("+QNWPREFCFG:"))
                        .ok_or_else(|| anyhow::anyhow!("missing network preference response"))?;
                    (
                        v.contains("AUTO") || v.contains("WCDMA"),
                        v.contains("AUTO") || v.contains("LTE"),
                        v.contains("AUTO") || v.contains("NR5G"),
                    )
                }
                Family::TdtechMt5700 => {
                    let v = raw
                        .lines()
                        .find_map(|l| {
                            l.strip_prefix("^SYSCFGEX:")
                                .and_then(|v| v.split('"').nth(1))
                        })
                        .ok_or_else(|| anyhow::anyhow!("missing SYSCFGEX response"))?;
                    (
                        v.contains("00") || v.contains("02"),
                        v.contains("00") || v.contains("03"),
                        v.contains("00") || v.contains("08"),
                    )
                }
            };
            json!({"network_prefer":{"3G":if g3{"1"}else{"0"},"4G":if g4{"1"}else{"0"},"5G":if g5{"1"}else{"0"}}})
        }
        Operation::GetSimSlot => {
            let slot = sim::parse_slot(raw)
                .ok_or_else(|| anyhow::anyhow!("missing valid SIM slot response"))?;
            json!({"sim_slot":slot,"source":"modem","hardware_verified":true})
        }
        Operation::Get5gLan => {
            let value = raw
                .lines()
                .find_map(|l| {
                    l.strip_prefix("+QCFG:")
                        .and_then(|v| v.trim().strip_prefix("\"5glan\",1,"))
                })
                .ok_or_else(|| anyhow::anyhow!("missing 5G LAN response"))?;
            ensure!(
                value.trim() == "0" || value.trim() == "1",
                "invalid 5G LAN state"
            );
            json!({"supported":true,"enabled":value.trim()=="1"})
        }
        _ => json!({}),
    };
    Ok(json!({"success":true,"data":data,"response":raw}))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn device(vendor: &str, platform: &str) -> Modem {
        serde_json::from_value(json!({"id":"m1","name":"test","manufacturer":vendor,"model":"mt5700m-cn","platform":platform,"at_port":"/dev/ttyUSB2","bus":"usb","apn":""})).unwrap()
    }
    fn bytes(d: &Modem, o: Operation) -> String {
        String::from_utf8(prepare(d, &o).unwrap().bytes).unwrap()
    }
    #[test]
    fn other_vendors_and_non_mt5700_tdtech_models_are_excluded() {
        assert!(family(&device("fibocom", "qualcomm")).is_err());
        assert!(family(&device("huawei", "hisilicon")).is_err());
        let mut d = device("tdtech", "hisilicon");
        d.model = "mh5000".into();
        assert!(family(&d).is_err());
    }
    #[test]
    fn quectel_platform_modes_match_upstream() {
        for platform in ["qualcomm", "unisoc", "lte12", "lte"] {
            assert_eq!(
                bytes(
                    &device("quectel", platform),
                    Operation::SetMode { mode: "ncm".into() }
                ),
                "AT+QCFG=\"usbnet\",5\r\n"
            );
        }
        assert_eq!(
            bytes(
                &device("quectel", "hisilicon"),
                Operation::SetMode { mode: "ncm".into() }
            ),
            "AT+QCFG=\"usbnet\",0\r\n"
        );
        assert_eq!(
            bytes(
                &device("tdtech", "hisilicon"),
                Operation::SetMode { mode: "ncm".into() }
            ),
            "AT^SETMODE=4\r\n"
        );
    }
    #[test]
    fn preference_commands_preserve_platform_and_upstream_pair_semantics() {
        let op = || Operation::SetNetworkPrefer {
            networks: vec!["4G".into(), "5G".into()],
        };
        assert_eq!(
            bytes(&device("quectel", "qualcomm"), op()),
            "AT+QNWPREFCFG=\"mode_pref\",LTE:NR5G\r\n"
        );
        assert_eq!(
            bytes(&device("tdtech", "hisilicon"), op()),
            "AT^SYSCFGEX=\"03\",40000000,1,2,40000000,,\r\n"
        );
        assert_eq!(
            bytes(
                &device("quectel", "lte"),
                Operation::SetNetworkPrefer {
                    networks: vec!["4G".into()]
                }
            ),
            "AT+QCFG=\"nwscanmode\",3\r\n"
        );
    }
    #[test]
    fn modem_errors_are_never_reported_as_success() {
        let r = Reply {
            status: 0,
            terminal: "ERROR".into(),
            modem_success: false,
            response: "ERROR\r\n".into(),
        };
        assert_eq!(
            interpret(&device("quectel", "qualcomm"), &Operation::GetImei, &r).unwrap()["success"],
            false
        );
    }
    #[test]
    fn malformed_successful_query_does_not_invent_default_values() {
        let r = Reply {
            status: 0,
            terminal: "OK".into(),
            modem_success: true,
            response: "OK\r\n".into(),
        };
        assert!(interpret(&device("quectel", "qualcomm"), &Operation::GetMode, &r).is_err());
    }
}

#[cfg(test)]
mod migration_tests;
