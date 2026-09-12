// SPDX-License-Identifier: GPL-3.0-only
// Vendor AT dial order derived from FUjr/QModem modem_dial.sh.
use crate::{
    at::{AtError, ErrorKind, Next, PortPool, Program, Reply, Step},
    config::Modem,
    vendor::{self, Family},
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
    time::Duration,
};
use tokio::sync::Mutex;
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub auto_connect: bool,
    pub driver: Driver,
    pub control_port: Option<String>,
    pub logical_interface: Option<String>,
    pub pdp_type: Pdp,
    pub metric: u32,
    pub mtu: Option<u16>,
    pub dns: Vec<std::net::IpAddr>,
    pub peer_dns: bool,
    pub default_route: bool,
    pub delegate: bool,
    pub modem_nat: bool,
    pub credentials: Credentials,
    pub sim2: Option<Credentials>,
    pub pre_dial_commands: Vec<String>,
}
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Driver {
    #[default]
    At,
    Qmi,
    Mbim,
}
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Pdp {
    Ip,
    Ipv6,
    #[default]
    Ipv4v6,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Credentials {
    pub apn: String,
    pub username: String,
    pub password: String,
    pub auth: String,
    pub pin: String,
}
impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field(
                "configured",
                &(!self.apn.is_empty() || !self.username.is_empty()),
            )
            .finish()
    }
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            auto_connect: false,
            driver: Driver::At,
            control_port: None,
            logical_interface: None,
            pdp_type: Pdp::Ipv4v6,
            metric: 50,
            mtu: None,
            dns: vec![],
            peer_dns: true,
            default_route: true,
            delegate: true,
            modem_nat: true,
            credentials: Default::default(),
            sim2: None,
            pre_dial_commands: vec![],
        }
    }
}
fn safe_at_string(s: &str) -> bool {
    s.len() <= 128
        && !s
            .chars()
            .any(|c| c.is_control() || ['"', '\\'].contains(&c))
}
impl Settings {
    pub fn validate(&self) -> Result<()> {
        if let Some(name) = &self.logical_interface {
            ensure!(
                !name.is_empty()
                    && name.len() <= 48
                    && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'),
                "logical interface must contain letters, digits or underscores"
            );
        }
        if let Some(path) = &self.control_port {
            ensure!(
                path.starts_with("/dev/") && !path.split('/').any(|s| s == ".."),
                "control port must be under /dev"
            );
        }
        ensure!(
            self.mtu.is_none_or(|v| (1280..=9000).contains(&v)),
            "MTU must be 1280 to 9000"
        );
        ensure!(self.dns.len() <= 8, "at most 8 DNS servers");
        for c in std::iter::once(&self.credentials).chain(self.sim2.iter()) {
            ensure!(
                [&c.apn, &c.username, &c.password]
                    .iter()
                    .all(|s| safe_at_string(s)),
                "invalid APN or credentials"
            );
            ensure!(
                ["", "none", "pap", "chap", "both", "auto", "MsChapV2"].contains(&c.auth.as_str()),
                "invalid authentication method"
            );
            ensure!(
                c.pin.is_empty()
                    || ((4..=8).contains(&c.pin.len())
                        && c.pin.bytes().all(|b| b.is_ascii_digit())),
                "PIN must be 4 to 8 digits"
            );
        }
        ensure!(
            self.pre_dial_commands.len() <= 16,
            "at most 16 pre-dial commands"
        );
        for c in &self.pre_dial_commands {
            Step::command(c, Duration::from_secs(10))?;
        }
        Ok(())
    }
}
impl Pdp {
    fn at(self) -> &'static str {
        match self {
            Self::Ip => "IP",
            Self::Ipv6 => "IPV6",
            Self::Ipv4v6 => "IPV4V6",
        }
    }
    fn netifd(self) -> &'static str {
        match self {
            Self::Ip => "ipv4",
            Self::Ipv6 => "ipv6",
            Self::Ipv4v6 => "ipv4v6",
        }
    }
}
pub fn interface_name(modem: &Modem) -> String {
    modem.network.logical_interface.clone().unwrap_or_else(|| {
        format!(
            "qm_{}_{}",
            modem
                .id
                .chars()
                .take(20)
                .collect::<String>()
                .replace('-', "_"),
            &crate::auth::digest(&modem.id)[..8]
        )
    })
}

fn credentials(modem: &Modem, slot: Option<u8>) -> Credentials {
    let mut c = modem.network.credentials.clone();
    if c.apn.is_empty() {
        c.apn = modem.apn.clone();
    }
    if slot == Some(2)
        && let Some(second) = &modem.network.sim2
    {
        if !second.apn.is_empty() {
            c.apn = second.apn.clone();
        }
        if !second.username.is_empty() {
            c.username = second.username.clone();
        }
        if !second.password.is_empty() {
            c.password = second.password.clone();
        }
        if !second.auth.is_empty() {
            c.auth = second.auth.clone();
        }
        if !second.pin.is_empty() {
            c.pin = second.pin.clone();
        }
    }
    c
}
fn cmd(c: &str) -> Step {
    Step::command(c, Duration::from_secs(30)).expect("validated dial command")
}
struct Dial {
    modem: Modem,
    credentials: Credentials,
    stage: u8,
    tail: VecDeque<Step>,
    pin_attempted: bool,
}
impl Dial {
    fn new(modem: Modem, credentials: Credentials) -> Self {
        Self {
            modem,
            credentials,
            stage: 0,
            tail: VecDeque::new(),
            pin_attempted: false,
        }
    }
    fn tail(&mut self, plmn: &str) {
        let pdp = self.modem.pdp_index;
        let c = &self.credentials;
        let n = &self.modem.network;
        let quectel = self.modem.manufacturer.eq_ignore_ascii_case("quectel");
        if !(quectel && self.modem.platform == "hisilicon") {
            self.tail.push_back(cmd(&format!(
                "AT+CGDCONT={pdp},\"{}\"{}",
                n.pdp_type.at(),
                if c.apn.is_empty() {
                    String::new()
                } else {
                    format!(",\"{}\"", c.apn)
                }
            )));
        }
        if !quectel && !c.auth.is_empty() {
            let auth = match c.auth.as_str() {
                "pap" => 1,
                "chap" => 2,
                "auto" | "both" | "MsChapV2" => 3,
                _ => 0,
            };
            if !c.username.is_empty() || (!c.password.is_empty() && auth != 0) {
                self.tail.push_back(cmd(&format!(
                    "AT^AUTHDATA={pdp},{auth},{plmn},\"{}\",\"{}\"",
                    c.username, c.password
                )));
            }
        }
        if quectel {
            self.tail
                .push_back(cmd(&format!("AT+QCFG=\"nat\",{}", u8::from(n.modem_nat))));
        }
        self.tail.push_back(cmd(&if quectel {
            match self.modem.platform.as_str() {
                "hisilicon" => "AT+QNETDEVCTL=1,1,1".into(),
                "unisoc" => format!("AT+QNETDEVCTL=1,{pdp},1"),
                _ => format!("AT+QNETDEVCTL=3,{pdp},1"),
            }
        } else {
            format!("AT^NDISDUP=1,{pdp}")
        }));
    }
}
impl Program for Dial {
    fn next(&mut self, replies: &[Reply]) -> std::result::Result<Next, AtError> {
        match self.stage {
            0 => {
                self.stage = 1;
                Ok(Next::Command(cmd("AT+CPIN?")))
            }
            1 => {
                let response = &replies.last().unwrap().response;
                if response.contains("+CPIN: SIM PIN")
                    && !self.credentials.pin.is_empty()
                    && !self.pin_attempted
                {
                    self.pin_attempted = true;
                    self.stage = 2;
                    return Ok(Next::Command(cmd(&format!(
                        "AT+CPIN=\"{}\"",
                        self.credentials.pin
                    ))));
                }
                if !response.contains("+CPIN: READY") {
                    return Err(AtError {
                        kind: ErrorKind::State,
                        message: "SIM is not ready; PIN retries are not automatic".into(),
                    });
                }
                self.stage = 4;
                for c in &self.modem.network.pre_dial_commands {
                    self.tail.push_back(cmd(c));
                }
                self.tail.push_back(cmd("AT+COPS=0,0"));
                self.next(replies)
            }
            2 => {
                if !replies.last().unwrap().modem_success {
                    return Err(AtError {
                        kind: ErrorKind::State,
                        message: "SIM PIN was rejected; automatic retries stopped".into(),
                    });
                }
                self.stage = 3;
                Ok(Next::Wait(Duration::from_secs(1)))
            }
            3 => {
                self.stage = 1;
                Ok(Next::Command(cmd("AT+CPIN?")))
            }
            4 => {
                if let Some(step) = self.tail.pop_front() {
                    return Ok(Next::Command(step));
                }
                if !self.modem.manufacturer.eq_ignore_ascii_case("quectel")
                    && !self.credentials.auth.is_empty()
                {
                    self.stage = 5;
                    Ok(Next::Command(cmd("AT+COPS=3,2;+COPS?")))
                } else {
                    self.tail("00000");
                    self.stage = 6;
                    self.next(replies)
                }
            }
            5 => {
                let plmn = replies
                    .last()
                    .unwrap()
                    .response
                    .lines()
                    .find_map(|l| l.trim().strip_prefix("+COPS:"))
                    .map(vendor::cells::fields)
                    .and_then(|f| f.get(2).cloned())
                    .map(|s| s.chars().take(5).collect::<String>())
                    .filter(|s| s.len() == 5 && s.bytes().all(|b| b.is_ascii_digit()))
                    .unwrap_or_else(|| "00000".into());
                self.tail(&plmn);
                self.stage = 6;
                self.next(replies)
            }
            _ => Ok(self.tail.pop_front().map_or(Next::Finish, Next::Command)),
        }
    }
}
/// netifd owns DHCP, QMI/MBIM session negotiation, DNS and IPv6 lifetimes.
/// TOML remains the source of truth; interfaces are added dynamically via ubus.
pub fn plan(modem: &Modem, c: &Credentials) -> Result<Value> {
    modem.network.validate()?;
    let n = &modem.network;
    let name = interface_name(modem);
    let mut p = json!({"name":name,"proto":match n.driver{Driver::At=>if n.pdp_type==Pdp::Ipv6{"dhcpv6"}else{"dhcp"},Driver::Qmi=>"qmi",Driver::Mbim=>"mbim"},"metric":n.metric,"defaultroute":n.default_route,"peerdns":n.peer_dns,"dns":n.dns,"delegate":n.delegate});
    if let Some(mtu) = n.mtu {
        p["mtu"] = json!(mtu);
    }
    match n.driver {
        Driver::At => {
            let device = modem
                .interface
                .as_deref()
                .context("AT dialing requires a data network interface")?;
            crate::config::validate_interface(device)?;
            ensure!(!device.is_empty(), "data interface cannot be empty");
            p["device"] = json!(device);
        }
        _ => {
            let control = n
                .control_port
                .as_deref()
                .context("QMI/MBIM requires control_port")?;
            p["device"] = json!(control);
            p["apn"] = json!(c.apn);
            p["username"] = json!(c.username);
            p["password"] = json!(c.password);
            p["auth"] = json!(if c.auth == "auto" { "both" } else { &c.auth });
            p["pincode"] = json!(c.pin);
            p["pdptype"] = json!(n.pdp_type.netifd());
            p["profile"] = json!(modem.pdp_index);
        }
    }
    Ok(p)
}
async fn ubus(object: &str, method: &str, data: &Value) -> Result<Value> {
    ensure!(
        std::path::Path::new("/etc/openwrt_release").is_file(),
        "network control requires OpenWrt 24.10 or later"
    );
    let mut command = tokio::process::Command::new("ubus");
    command
        .args(["-t", "30", "call", object, method, &data.to_string()])
        .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(35), command.output())
        .await
        .context("ubus timed out")??;
    ensure!(
        output.status.success(),
        "netifd {method} failed for {object}"
    );
    if output.stdout.is_empty() {
        Ok(json!({}))
    } else {
        Ok(serde_json::from_slice(&output.stdout)?)
    }
}
#[derive(Default)]
pub struct Manager {
    locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    pub states: Mutex<HashMap<String, Value>>,
}
impl Manager {
    pub async fn lock(&self, id: &str) -> Arc<Mutex<()>> {
        self.locks
            .lock()
            .await
            .entry(id.into())
            .or_default()
            .clone()
    }
    pub async fn operate(
        &self,
        modem: Modem,
        pool: PortPool,
        runtime: vendor::Runtime,
        operation: &str,
    ) -> Result<Value> {
        let lock = self.lock(&modem.id).await;
        let _guard = lock.lock().await;
        ensure!(
            ["connect", "disconnect", "redial", "status", "plan"].contains(&operation),
            "unknown network operation"
        );
        self.operate_locked(modem, pool, runtime, operation).await
    }
    pub async fn operate_locked(
        &self,
        modem: Modem,
        pool: PortPool,
        runtime: vendor::Runtime,
        operation: &str,
    ) -> Result<Value> {
        let family = vendor::family(&modem)?;
        let slot = if family == Family::TdtechMt5700 {
            Some(runtime.slot(&modem.id)?)
        } else {
            None
        };
        let slot = if operation != "plan"
            && ["connect", "redial"].contains(&operation)
            && family == Family::Quectel
            && modem.network.sim2.is_some()
        {
            let port = pool.get(&modem.at_port).await?;
            let replies = port
                .run_named(
                    Box::new(crate::at::Sequence::new(vec![cmd("AT+QUIMSLOT?")], false)),
                    Some(modem.id.clone()),
                    "network_sim_slot",
                )
                .await?;
            vendor::finish(&modem, &vendor::Operation::GetSimSlot, &replies)?
                .get("data")
                .and_then(|v| v["sim_slot"].as_u64())
                .map(|v| v as u8)
        } else {
            slot
        };
        let c = credentials(&modem, slot);
        let plan = plan(&modem, &c)?;
        if operation == "plan" {
            let mut plan = plan;
            for field in ["password", "pincode"] {
                if plan.get(field).is_some() {
                    plan[field] = json!("***");
                }
            }
            return Ok(json!({"interface":plan,"hardware_verified":false}));
        }
        let object = format!("network.interface.{}", interface_name(&modem));
        if operation == "status" {
            return ubus(&object, "status", &json!({})).await;
        }
        ensure!(
            std::path::Path::new("/etc/openwrt_release").is_file(),
            "network control requires OpenWrt 24.10 or later"
        );
        if let Ok(existing) = ubus(&object, "status", &json!({})).await {
            ensure!(
                existing["dynamic"] == true,
                "refusing to replace a static OpenWrt interface"
            );
        }
        self.states
            .lock()
            .await
            .insert(modem.id.clone(), json!({"state":operation}));
        let result=async {
   if ["disconnect","redial"].contains(&operation){
    let down=ubus(&object,"down",&json!({})).await;
    if modem.network.driver==Driver::At && modem.network.pdp_type==Pdp::Ipv4v6 { let _=ubus(&format!("{object}v6"),"down",&json!({})).await; }
    if modem.network.driver==Driver::At{
     let hang=if family==Family::Quectel{format!("AT+QNETDEVCTL={},2,1",modem.pdp_index)}else{"AT^NDISDUP=0,0".into()};
     let replies=pool.get(&modem.at_port).await?.run_named(Box::new(crate::at::Sequence::new(vec![cmd(&hang)],false)),Some(modem.id.clone()),"network_disconnect").await?;
     ensure!(replies.iter().all(|r|r.modem_success),"modem rejected disconnect");
    }
    if operation=="disconnect"{down?;return Ok(json!({"state":"disconnected"}));}
   }
   if modem.network.driver==Driver::At{
    let replies=pool.get(&modem.at_port).await?.run_named(Box::new(Dial::new(modem.clone(),c)),Some(modem.id.clone()),"network_connect").await?;
    ensure!(replies.last().is_some_and(|r|r.modem_success),"modem rejected dial command");
   }
   ubus("network","add_dynamic",&plan).await?;
   if modem.network.driver==Driver::At&&modem.network.pdp_type==Pdp::Ipv4v6{
    ubus("network","add_dynamic",&json!({"name":format!("{}v6",interface_name(&modem)),"proto":"dhcpv6","device":modem.interface,"metric":modem.network.metric,"delegate":modem.network.delegate,"peerdns":modem.network.peer_dns,"dns":modem.network.dns})).await?;
   }
   Ok::<_,anyhow::Error>(json!({"state":"connecting","interface":interface_name(&modem),"ip_acquired":false}))
  }.await;
        self.states.lock().await.insert(
            modem.id.clone(),
            match &result {
                Ok(v) => v.clone(),
                Err(e) => json!({"state":"failed","error":e.to_string()}),
            },
        );
        result
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn modem(platform: &str) -> Modem {
        serde_json::from_value(json!({"id":"m1","name":"test","manufacturer":"quectel","platform":platform,"at_port":"/dev/test","interface":"wwan0","bus":"usb"})).unwrap()
    }
    #[test]
    fn dial_sequences_preserve_vendor_order() {
        for (platform, expected) in [
            ("qualcomm", "AT+QNETDEVCTL=3,1,1\r\n"),
            ("unisoc", "AT+QNETDEVCTL=1,1,1\r\n"),
            ("hisilicon", "AT+QNETDEVCTL=1,1,1\r\n"),
        ] {
            let m = modem(platform);
            let mut program = Dial::new(
                m,
                Credentials {
                    apn: "internet".into(),
                    ..Default::default()
                },
            );
            let mut replies = vec![];
            let mut commands = vec![];
            loop {
                match program.next(&replies).unwrap() {
                    Next::Command(s) => {
                        commands.push(String::from_utf8(s.bytes).unwrap());
                        replies.push(Reply {
                            status: 0,
                            terminal: "OK".into(),
                            modem_success: true,
                            response: "+CPIN: READY\r\nOK".into(),
                        });
                    }
                    Next::Finish => break,
                    _ => panic!(),
                }
            }
            assert_eq!(commands[0], "AT+CPIN?\r\n");
            assert_eq!(commands[1], "AT+COPS=0,0\r\n");
            assert_eq!(commands.last().unwrap(), expected);
            assert_eq!(commands[commands.len() - 2], "AT+QCFG=\"nat\",1\r\n");
            assert_eq!(
                commands.iter().any(|c| c.contains("CGDCONT")),
                platform != "hisilicon"
            );
        }
    }
    #[test]
    fn netifd_plan_uses_24_10_protocol_options() {
        let mut m = modem("qualcomm");
        m.network.driver = Driver::Qmi;
        m.network.control_port = Some("/dev/cdc-wdm0".into());
        let plan = plan(&m, &Credentials::default()).unwrap();
        assert_eq!(plan["proto"], "qmi");
        assert_eq!(plan["pdptype"], "ipv4v6");
        assert_eq!(plan["profile"], 1);
    }
    #[test]
    fn invalid_quoted_credentials_are_rejected() {
        let mut m = modem("qualcomm");
        m.network.credentials.apn = "evil\";AT+CFUN=0".into();
        assert!(m.network.validate().is_err());
    }
}
