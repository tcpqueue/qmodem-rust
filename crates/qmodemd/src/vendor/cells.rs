// SPDX-License-Identifier: GPL-3.0-only
// Ported from FUjr/QModem vendor/quectel.sh lockcell_* and get_neighborcell_*.
use super::*;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lock {
    pub rat: Rat,
    pub arfcn: u32,
    pub pci: Option<u16>,
    pub scs: Option<u8>,
    pub band: Option<u16>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Rat {
    Lte,
    Nr,
}
fn supported(device: &Modem) -> Result<()> {
    ensure!(
        family(device)? == Family::Quectel
            && ["qualcomm", "lte12", "lte", "unisoc"].contains(&device.platform.as_str()),
        "cell locking is unavailable on this profile"
    );
    Ok(())
}
fn cmd(command: String) -> Result<Step> {
    Step::command(&command, Duration::from_secs(10))
}
pub fn lock(device: &Modem, lock: &Lock) -> Result<Step> {
    supported(device)?;
    ensure!(lock.arfcn <= 3_279_165, "ARFCN is out of range");
    ensure!(
        lock.rat != Rat::Lte || lock.arfcn <= 262143,
        "LTE EARFCN is out of range"
    );
    ensure!(
        lock.pci
            .is_none_or(|n| n <= if lock.rat == Rat::Lte { 503 } else { 1007 }),
        "PCI is out of range"
    );
    let command = match device.platform.as_str() {
        "lte" => {
            ensure!(lock.rat == Rat::Lte, "LTE profile cannot lock NR");
            format!(
                "AT+QNWLOCK=\"common/lte\",{},{},{}",
                if lock.pci.is_some() { 2 } else { 1 },
                lock.arfcn,
                lock.pci.unwrap_or(0)
            )
        }
        "unisoc" => {
            let pci = lock.pci.context("PCI is required")?;
            format!(
                "AT+QNWLOCK=\"common/{}\",1,{},{}",
                if lock.rat == Rat::Nr { "5g" } else { "lte" },
                lock.arfcn,
                pci
            )
        }
        _ => {
            let pci = lock.pci.context("PCI is required")?;
            if lock.rat == Rat::Lte {
                format!("AT+QNWLOCK=\"common/4g\",1,{},{pci}", lock.arfcn)
            } else {
                let scs = lock.scs.context("NR SCS index is required")?;
                ensure!(scs <= 5, "NR SCS index must be 0 through 5");
                let band = lock.band.context("NR band is required")?;
                ensure!((1..=1024).contains(&band), "invalid NR band");
                format!(
                    "AT+QNWLOCK=\"common/5g\",{pci},{},{},{band}",
                    lock.arfcn,
                    15u32 << scs
                )
            }
        }
    };
    cmd(command)
}
fn lte_class(device: &Modem) -> &'static str {
    if ["qualcomm", "lte12"].contains(&device.platform.as_str()) {
        "common/4g"
    } else {
        "common/lte"
    }
}
pub fn unlock(device: &Modem) -> Result<Vec<Step>> {
    supported(device)?;
    let mut steps = vec![];
    if device.platform != "lte" {
        steps.push(cmd("AT+QNWLOCK=\"common/5g\",0".into())?);
    }
    steps.push(cmd(format!("AT+QNWLOCK=\"{}\",0", lte_class(device)))?);
    Ok(steps)
}
pub fn query(device: &Modem) -> Result<Vec<Step>> {
    supported(device)?;
    let mut steps = vec![cmd(format!("AT+QNWLOCK=\"{}\"", lte_class(device)))?];
    if device.platform != "lte" {
        steps.push(cmd("AT+QNWLOCK=\"common/5g\"".into())?);
    }
    steps.push(cmd("AT+QENG=\"neighbourcell\"".into())?);
    Ok(steps)
}
use anyhow::Context;
/// Quote-aware AT CSV parser shared by status and neighbour reports.
pub fn fields(line: &str) -> Vec<String> {
    let mut values = vec![];
    let mut part = String::new();
    let mut quoted = false;
    for c in line.chars() {
        match c {
            '"' => quoted = !quoted,
            ',' if !quoted => {
                values.push(part.trim().into());
                part.clear();
            }
            _ => part.push(c),
        }
    }
    values.push(part.trim().into());
    values
}
pub fn interpret(device: &Modem, replies: &[Reply]) -> Result<Value> {
    let expected = if device.platform == "lte" { 2 } else { 3 };
    ensure!(
        replies.len() == expected,
        "incomplete neighbour transaction"
    );
    let locks=replies[..expected-1].iter().enumerate().map(|(index,reply)| {
        let raw=reply.response.lines().find_map(|l| l.trim().strip_prefix("+QNWLOCK:")).map(fields);
        let Some(f)=raw else {return json!({"rat":if index==0{"lte"}else{"nr"},"known":false,"locked":null});};
        let field=|i:usize| f.get(i).map(String::as_str).unwrap_or("");
        let (locked,arfcn,pci)=if device.platform=="unisoc" {
            if index==0 {(!field(1).is_empty(),field(1),field(2))} else {(!field(2).is_empty(),field(2),field(1))}
        }else if index==0 {(field(1)!="0",field(2),field(3))} else {(field(1)!="0",field(2),field(1))};
        json!({"rat":if index==0{"lte"}else{"nr"},"known":reply.modem_success,"locked":if reply.modem_success{Some(locked)}else{None},"arfcn":arfcn,"pci":pci,"scs":if index==1{f.get(3)}else{None},"band":if index==1{f.get(4)}else{None},"fields":f})
    }).collect::<Vec<_>>();
    let neighbors=replies.last().unwrap().response.lines().filter_map(|line| {
        let fields=fields(line.trim().strip_prefix("+QENG:")?);
        let get=|i:usize| fields.get(i).cloned();
        // Preserve upstream top-level field ordering, including its RSRP/RSRQ assignment.
        Some(json!({"relation":get(0),"rat":get(1),"arfcn":get(2),"pci":get(3),"rsrp":if get(1).as_deref()==Some("WCDMA"){None}else{get(if device.platform=="lte"{5}else{4})},"rsrq":if get(1).as_deref()==Some("WCDMA"){None}else{get(if device.platform=="lte"{4}else{5})},"rscp":if get(1).as_deref()==Some("WCDMA"){get(5)}else{None},"ecno":if get(1).as_deref()==Some("WCDMA"){get(6)}else{None},"fields":fields}))
    }).collect::<Vec<_>>();
    let usable = replies.iter().any(|r| r.modem_success);
    Ok(
        json!({"success":usable,"data":{"locks":locks,"neighbors":neighbors,"partial":replies.iter().any(|r|!r.modem_success)},"replies":replies}),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    fn modem(platform: &str) -> Modem {
        serde_json::from_value(json!({"id":"m","name":"test","manufacturer":"quectel","platform":platform,"at_port":"/dev/test","bus":"usb"})).unwrap()
    }
    #[test]
    fn platform_command_order_matches_upstream() {
        let lock = Lock {
            rat: Rat::Nr,
            arfcn: 627264,
            pci: Some(113),
            scs: Some(1),
            band: Some(78),
        };
        assert_eq!(
            self::lock(&modem("qualcomm"), &lock).unwrap().bytes,
            b"AT+QNWLOCK=\"common/5g\",113,627264,30,78\r\n"
        );
        assert_eq!(
            self::lock(&modem("unisoc"), &lock).unwrap().bytes,
            b"AT+QNWLOCK=\"common/5g\",1,627264,113\r\n"
        );
        let steps = unlock(&modem("qualcomm")).unwrap();
        assert_eq!(steps[0].bytes, b"AT+QNWLOCK=\"common/5g\",0\r\n");
        assert_eq!(steps[1].bytes, b"AT+QNWLOCK=\"common/4g\",0\r\n");
        let lock = Lock {
            rat: Rat::Lte,
            arfcn: 1850,
            pci: None,
            scs: None,
            band: None,
        };
        assert_eq!(
            self::lock(&modem("lte"), &lock).unwrap().bytes,
            b"AT+QNWLOCK=\"common/lte\",1,1850,0\r\n"
        );
        assert!(self::lock(&modem("qualcomm"), &lock).is_err());
    }
}
