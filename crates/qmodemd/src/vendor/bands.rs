// SPDX-License-Identifier: GPL-3.0-only
// AT behavior ported from FUjr/QModem vendor/quectel.sh.
// Copyright (C) 2023 Siriling <siriling@qq.com>
// Copyright (C) 2025 Fujr <fjrcn@outlook.com>
// Rust adaptation Copyright (C) 2026 tcpqueue
use super::*;
use serde::Serialize;
use std::sync::LazyLock;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
pub enum Class {
    #[serde(rename = "UMTS")]
    Umts,
    #[serde(rename = "LTE")]
    Lte,
    #[serde(rename = "NR_NSA")]
    Nsa,
    #[serde(rename = "NR")]
    Nr,
}
impl Class {
    fn field(self) -> &'static str {
        match self {
            Self::Umts => "gw_band",
            Self::Lte => "lte_band",
            Self::Nsa => "nsa_nr5g_band",
            Self::Nr => "nr5g_band",
        }
    }
    fn catalog_key(self) -> &'static str {
        match self {
            Self::Umts => "wcdma_band",
            Self::Lte => "lte_band",
            Self::Nsa => "nsa_band",
            Self::Nr => "sa_band",
        }
    }
    fn defaults(self) -> &'static str {
        match self {
            Self::Umts => "1/2/3/4/5/6/7/8/9/19",
            Self::Lte => {
                "1/2/3/4/5/7/8/12/13/14/17/18/19/20/25/26/28/29/30/32/34/38/39/40/41/42/66/71"
            }
            Self::Nsa => "1/2/3/5/7/8/12/20/25/28/38/40/41/48/66/71/77/78/79/257/258/260/261",
            Self::Nr => "1/2/3/5/7/8/12/20/25/28/38/40/41/48/66/71/77/78/79",
        }
    }
}
const CLASSES: [Class; 4] = [Class::Umts, Class::Lte, Class::Nsa, Class::Nr];
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Overrides {
    pub umts: Option<Vec<u16>>,
    pub lte: Option<Vec<u16>>,
    pub nr_nsa: Option<Vec<u16>>,
    pub nr: Option<Vec<u16>>,
}
impl Overrides {
    pub fn validate(&self) -> Result<()> {
        for class in CLASSES {
            if let Some(bands) = self.get(class) {
                validate(bands)?;
            }
        }
        Ok(())
    }
    fn get(&self, class: Class) -> Option<&Vec<u16>> {
        match class {
            Class::Umts => self.umts.as_ref(),
            Class::Lte => self.lte.as_ref(),
            Class::Nsa => self.nr_nsa.as_ref(),
            Class::Nr => self.nr.as_ref(),
        }
    }
}
pub fn validate(bands: &[u16]) -> Result<()> {
    ensure!(
        bands.len() <= 1024 && bands.iter().all(|b| (1..=1024).contains(b)),
        "bands must be between 1 and 1024, with at most 1024 entries"
    );
    Ok(())
}
pub fn query(device: &Modem) -> Result<Vec<Step>> {
    ensure!(
        family(device)? == Family::Quectel,
        "band locking is disabled in the MT5700 upstream profile"
    );
    let commands = if ["qualcomm", "unisoc", "lte12"].contains(&device.platform.as_str()) {
        CLASSES
            .iter()
            .map(|c| format!("AT+QNWPREFCFG=\"{}\"", c.field()))
            .collect()
    } else {
        vec!["AT+QCFG=\"band\"".into()]
    };
    commands
        .iter()
        .map(|c| Step::command(c, Duration::from_secs(10)))
        .collect()
}
pub fn setter(device: &Modem, class: Class, bands: &[u16]) -> Result<Step> {
    ensure!(
        family(device)? == Family::Quectel,
        "band locking is disabled in the MT5700 upstream profile"
    );
    validate(bands)?;
    // Upstream dispatches only the exact 'lte' platform to the hexadecimal setter.
    let command = if device.platform == "lte" {
        format!("AT+QCFG=\"band\",0,{},0", mask(bands))
    } else {
        format!(
            "AT+QNWPREFCFG=\"{}\",{}",
            class.field(),
            bands
                .iter()
                .map(u16::to_string)
                .collect::<Vec<_>>()
                .join(":")
        )
    };
    Step::command(&command, Duration::from_secs(10))
}
fn mask(bands: &[u16]) -> String {
    let mut digits = vec![0u8; usize::from(bands.iter().copied().max().unwrap_or(1)).div_ceil(4)];
    for band in bands {
        let bit = usize::from(*band - 1);
        digits[bit / 4] |= 1 << (bit % 4);
    }
    digits
        .into_iter()
        .rev()
        .map(|d| char::from(b"0123456789ABCDEF"[usize::from(d)]))
        .collect()
}
fn parse_mask(value: &str) -> Result<Vec<u16>> {
    let value = value.trim().trim_matches('"');
    ensure!(
        !value.is_empty() && value.len() <= 256,
        "invalid LTE band mask length"
    );
    let mut bands = Vec::new();
    for (index, c) in value.chars().rev().enumerate() {
        let digit = c
            .to_digit(16)
            .ok_or_else(|| anyhow::anyhow!("invalid LTE band mask"))?;
        for bit in 0..4 {
            if digit & (1 << bit) != 0 {
                bands.push((index * 4 + bit + 1) as u16);
            }
        }
    }
    Ok(bands)
}
static CATALOG: LazyLock<Value> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../../../../data/supported-models.json"))
        .expect("bundled catalog")
});
fn available(device: &Modem, class: Class, legacy: bool) -> (Vec<u16>, &'static str) {
    if let Some(bands) = device.bands.get(class) {
        return (bands.clone(), "configuration");
    }
    let bus = match device.bus {
        crate::config::Bus::Usb => "usb",
        crate::config::Bus::Pcie => "pcie",
    };
    // Legacy upstream UI ignored model overrides and had misspelled calls for B7/B40.
    // The redesigned API uses the model's catalog when known, retaining raw AT masks.
    if let Some(raw) = CATALOG["modem_support"][bus][device.model.to_ascii_lowercase()]
        [class.catalog_key()]
    .as_str()
    {
        return (
            raw.split('/')
                .filter_map(|b| b.parse::<u16>().ok())
                .filter(|b| *b > 0)
                .collect(),
            "catalog",
        );
    }
    if legacy {
        return (
            vec![1, 3, 5, 7, 8, 20, 34, 38, 39, 40, 41],
            "legacy_defaults",
        );
    }
    (
        class
            .defaults()
            .split('/')
            .map(|b| b.parse().unwrap())
            .collect(),
        "defaults",
    )
}
fn parse_list(raw: &str, class: Class) -> Result<Vec<u16>> {
    let value = raw
        .lines()
        .find_map(|l| {
            let (name, value) = l.strip_prefix("+QNWPREFCFG:")?.trim().split_once(',')?;
            (name.trim_matches('"') == class.field()).then_some(value)
        })
        .ok_or_else(|| anyhow::anyhow!("missing {} response", class.field()))?;
    if value.trim().is_empty() {
        return Ok(vec![]);
    }
    let list = value
        .trim()
        .split(':')
        .map(|b| b.parse::<u16>())
        .collect::<std::result::Result<Vec<_>, _>>()?;
    // Some firmware returns 0 for an unsupported class; preserve it in raw reply,
    // but never offer it as a selectable band.
    ensure!(list.iter().all(|b| *b <= 1024), "band value exceeds limit");
    Ok(list)
}
pub fn interpret(device: &Modem, replies: &[Reply]) -> Result<Value> {
    let legacy = !["qualcomm", "unisoc", "lte12"].contains(&device.platform.as_str());
    ensure!(
        replies.len() == if legacy { 1 } else { 4 },
        "incomplete band transaction"
    );
    let mut entries = Vec::new();
    for (i, reply) in replies.iter().enumerate() {
        let class = if legacy { Class::Lte } else { CLASSES[i] };
        if !legacy
            && (device.platform == "lte12" && i >= 2
                || device.platform == "unisoc" && class == Class::Nsa)
        {
            continue;
        }
        let parsed = if !reply.modem_success {
            Err(anyhow::anyhow!("modem rejected query"))
        } else if legacy {
            reply
                .response
                .lines()
                .find_map(|l| l.strip_prefix("+QCFG:").and_then(|v| v.split(',').nth(2)))
                .ok_or_else(|| anyhow::anyhow!("missing LTE band mask"))
                .and_then(parse_mask)
        } else {
            parse_list(&reply.response, class)
        };
        let (available, source) = available(device, class, legacy);
        let entry = match parsed {
            Ok(locked) => {
                json!({"band_class":class,"state":"known","locked_bands":locked,"available_bands":available,"available_source":source})
            }
            Err(e) => {
                json!({"band_class":class,"state":"unknown","locked_bands":null,"available_bands":available,"available_source":source,"error":e.to_string()})
            }
        };
        entries.push(entry);
    }
    let known = entries.iter().filter(|e| e["state"] == "known").count();
    Ok(
        json!({"success":known>0,"error_code":if replies.iter().any(|r|!r.modem_success){"modem_rejected"}else{"invalid_modem_response"},
        "data":{"bands":entries,"partial":known<entries.len()},"replies":replies}),
    )
}
