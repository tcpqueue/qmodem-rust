use crate::{
    at::{PortPool, Sequence, Step},
    config::Modem,
    network, vendor,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub enabled: bool,
    pub interval_seconds: u64,
    pub threshold: u32,
    pub cooldown_seconds: u64,
    pub readiness_grace: u32,
    pub method: Method,
    pub target: String,
    pub ip_version: u8,
    pub actions: Vec<Action>,
}
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    #[default]
    Ping,
    Gateway,
    Dns,
    Http,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Redial,
    SwitchSim,
    At { commands: Vec<String> },
    Exec { path: String, args: Vec<String> },
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_seconds: 12,
            threshold: 5,
            cooldown_seconds: 60,
            readiness_grace: 5,
            method: Method::Ping,
            target: "1.1.1.1".into(),
            ip_version: 4,
            actions: vec![],
        }
    }
}
impl Settings {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (3..=86400).contains(&self.interval_seconds)
                && self.threshold > 0
                && self.threshold <= 1000,
            "invalid watchdog interval or threshold"
        );
        ensure!(
            [4, 6].contains(&self.ip_version)
                && self.cooldown_seconds <= 86400
                && self.readiness_grace <= 1000,
            "invalid watchdog IP version, cooldown or grace"
        );
        match self.method {
            Method::Ping => {
                ensure!(
                    self.target.parse::<std::net::IpAddr>().is_ok(),
                    "ping target must be an IP address"
                );
            }
            Method::Http => {
                let url = reqwest::Url::parse(&self.target)?;
                ensure!(
                    ["http", "https"].contains(&url.scheme()),
                    "watchdog URL must use HTTP(S)"
                );
            }
            _ => {}
        }
        ensure!(self.actions.len() <= 8, "at most 8 watchdog actions");
        for action in &self.actions {
            match action {
                Action::At { commands } => {
                    ensure!(commands.len() <= 16, "at most 16 watchdog AT commands");
                    for c in commands {
                        Step::command(c, Duration::from_secs(30))?;
                    }
                }
                Action::Exec { path, args } => {
                    ensure!(
                        Path::new(path).is_absolute()
                            && !path.contains('\0')
                            && args.len() <= 32
                            && args.iter().all(|a| a.len() <= 4096 && !a.contains('\0')),
                        "invalid custom action executable"
                    );
                }
                _ => {}
            }
        }
        Ok(())
    }
}
pub struct Counter {
    failures: u32,
    grace: u32,
    last_action: Option<Instant>,
}
impl Counter {
    pub fn new(settings: &Settings) -> Self {
        Self {
            failures: 0,
            grace: settings.readiness_grace,
            last_action: None,
        }
    }
    pub fn observe(
        &mut self,
        ready: bool,
        healthy: bool,
        settings: &Settings,
        now: Instant,
    ) -> (bool, Value) {
        if !ready && self.grace > 0 {
            self.grace -= 1;
            return (
                false,
                json!({"state":"waiting_for_interface","grace_remaining":self.grace,"failures":self.failures}),
            );
        }
        if healthy {
            self.failures = 0;
        } else {
            self.failures = self.failures.saturating_add(1);
        }
        let cooling = self.last_action.is_some_and(|last| {
            now.duration_since(last) < Duration::from_secs(settings.cooldown_seconds)
        });
        let trigger = self.failures >= settings.threshold && !cooling;
        if trigger {
            self.last_action = Some(now);
            self.failures = 0;
            self.grace = settings.readiness_grace;
        }
        (
            trigger,
            json!({"state":if healthy{"healthy"}else if cooling{"cooldown"}else{"unhealthy"},"failures":self.failures,"threshold":settings.threshold,"action_triggered":trigger}),
        )
    }
}
async fn ping(interface: &str, target: &str) -> bool {
    let mut cmd = tokio::process::Command::new("ping");
    cmd.args(["-c", "1", "-W", "3", "-I", interface, target])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    matches!(tokio::time::timeout(Duration::from_secs(5),cmd.status()).await,Ok(Ok(status))if status.success())
}
pub async fn check(
    modem: &Modem,
    manager: &network::Manager,
    pool: PortPool,
    runtime: vendor::Runtime,
) -> Result<(bool, bool)> {
    let interface = modem
        .interface
        .as_deref()
        .context("watchdog requires a data interface")?;
    crate::config::validate_interface(interface)?;
    if !Path::new("/sys/class/net").join(interface).exists() {
        return Ok((false, false));
    }
    let s = &modem.monitor;
    let healthy = match s.method {
        Method::Ping => ping(interface, &s.target).await,
        Method::Http => {
            let client = reqwest::Client::builder()
                .interface(interface)
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(15))
                .redirect(reqwest::redirect::Policy::none())
                .build()?;
            client
                .get(&s.target)
                .send()
                .await
                .is_ok_and(|r| r.status().is_success() || r.status().is_redirection())
        }
        Method::Gateway | Method::Dns => {
            let status = manager
                .operate(modem.clone(), pool, runtime, "status")
                .await
                .unwrap_or(Value::Null);
            let ipv6 = s.ip_version == 6;
            let target = match s.method {
                Method::Gateway => status["route"]
                    .as_array()
                    .and_then(|routes| {
                        routes
                            .iter()
                            .find(|r| r["target"] == if ipv6 { "::" } else { "0.0.0.0" })
                    })
                    .and_then(|r| r["nexthop"].as_str()),
                _ => status["dns-server"]
                    .as_array()
                    .and_then(|dns| {
                        dns.iter().find(|v| {
                            v.as_str().is_some_and(|ip| {
                                ip.parse::<std::net::IpAddr>()
                                    .is_ok_and(|ip| ip.is_ipv6() == ipv6)
                            })
                        })
                    })
                    .and_then(Value::as_str),
            };
            let mut success = false;
            if let Some(target) = target {
                success = ping(interface, target).await;
            }
            if !success {
                for target in if ipv6 {
                    ["2606:4700:4700::1111", "2001:4860:4860::8888"]
                } else {
                    ["1.1.1.1", "223.5.5.5"]
                } {
                    if ping(interface, target).await {
                        success = true;
                        break;
                    }
                }
            }
            success
        }
    };
    Ok((true, healthy))
}
pub async fn execute(path: &str, args: &[String]) -> Result<()> {
    let mut cmd = tokio::process::Command::new(path);
    cmd.args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let result = tokio::time::timeout(Duration::from_secs(60), cmd.status())
        .await
        .context("custom action timed out")??;
    ensure!(result.success(), "custom action failed");
    Ok(())
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TrafficSettings {
    pub enabled: bool,
    pub interval_seconds: u64,
    pub save_modem_counters: bool,
    pub retention_days: u32,
    pub reset: crate::schedule::ResetSchedule,
}
impl Default for TrafficSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_seconds: 300,
            save_modem_counters: false,
            retention_days: 90,
            reset: Default::default(),
        }
    }
}
impl TrafficSettings {
    pub fn validate(&self) -> Result<()> {
        self.reset.validate()?;
        ensure!(
            (10..=86400).contains(&self.interval_seconds) && self.retention_days <= 3650,
            "invalid traffic sampling settings"
        );
        Ok(())
    }
}
pub async fn traffic(
    modem: Modem,
    pool: PortPool,
    path: PathBuf,
    runtime: vendor::Runtime,
) -> Result<Value> {
    let op = vendor::Operation::GetUsageStats;
    let value = if let Some(local) = vendor::local(&modem, &op, &runtime)? {
        local
    } else {
        let port = pool.get(&modem.at_port).await?;
        let replies = port
            .run_named(
                vendor::plan(&modem, &op, &runtime)?,
                Some(modem.id.clone()),
                "traffic_sample",
            )
            .await?;
        vendor::finish(&modem, &op, &replies)?
    };
    let mut data = value["data"].clone();
    if data["available"] != true
        && let Some(interface) = &modem.interface
    {
        crate::config::validate_interface(interface)?;
        let base = Path::new("/sys/class/net")
            .join(interface)
            .join("statistics");
        let read = |name: &str| {
            std::fs::read_to_string(base.join(name))
                .ok()
                .and_then(|v| v.trim().parse::<u64>().ok())
        };
        if let (Some(rx), Some(tx)) = (read("rx_bytes"), read("tx_bytes")) {
            data = json!({"available":true,"total_rx_bytes":rx,"total_tx_bytes":tx,"source":"interface"});
        }
    } else {
        data["source"] = json!("modem");
    }
    if data["available"] == true {
        let id = modem.id.clone();
        let rx = data["total_rx_bytes"]
            .as_u64()
            .context("invalid RX counter")?;
        let tx = data["total_tx_bytes"]
            .as_u64()
            .context("invalid TX counter")?;
        ensure!(
            rx <= i64::MAX as u64 && tx <= i64::MAX as u64,
            "counter exceeds SQLite integer range"
        );
        let source = data["source"].as_str().unwrap_or("unknown").to_owned();
        let retention = modem.traffic.retention_days;
        crate::sms::database::run(path, move |db| {
            let now = crate::sms::database::now();
            db.execute(
                "INSERT OR REPLACE INTO traffic(modem_id,timestamp,rx_bytes,tx_bytes,source) VALUES (?,?,?,?,?)",
                rusqlite::params![id, now, rx, tx, source],
            )?;
            if retention > 0 {
                db.execute(
                    "DELETE FROM traffic WHERE modem_id=? AND timestamp<?",
                    rusqlite::params![id, now - i64::from(retention) * 86400],
                )?;
            }
            Ok(())
        })
        .await?;
    }
    if modem.traffic.save_modem_counters && vendor::family(&modem)? == vendor::Family::Quectel {
        let op = vendor::Operation::WriteUsageStats;
        let port = pool.get(&modem.at_port).await?;
        let replies = port
            .run_named(
                vendor::plan(&modem, &op, &runtime)?,
                Some(modem.id),
                "traffic_save",
            )
            .await?;
        data["modem_saved"] = json!(replies.iter().all(|r| r.modem_success));
    }
    Ok(data)
}
pub async fn send_at(modem: &Modem, pool: &PortPool, commands: &[String]) -> Result<()> {
    let steps = commands
        .iter()
        .map(|c| Step::command(c, Duration::from_secs(30)))
        .collect::<Result<Vec<_>>>()?;
    if steps.is_empty() {
        return Ok(());
    }
    let replies = pool
        .get(&modem.at_port)
        .await?
        .run_named(
            Box::new(Sequence::new(steps, true)),
            Some(modem.id.clone()),
            "watchdog_at",
        )
        .await?;
    ensure!(
        replies.iter().all(|r| r.modem_success),
        "watchdog AT command rejected"
    );
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn threshold_grace_and_cooldown_do_not_flap() {
        let s = Settings {
            threshold: 2,
            readiness_grace: 1,
            cooldown_seconds: 60,
            ..Default::default()
        };
        let mut counter = Counter::new(&s);
        let t = Instant::now();
        assert!(!counter.observe(false, false, &s, t).0);
        assert!(!counter.observe(false, false, &s, t).0);
        assert!(counter.observe(false, false, &s, t).0);
        assert!(
            !counter
                .observe(true, false, &s, t + Duration::from_secs(5))
                .0
        );
        assert!(
            !counter
                .observe(true, false, &s, t + Duration::from_secs(20))
                .0
        );
        assert!(
            counter
                .observe(true, false, &s, t + Duration::from_secs(61))
                .0
        );
        assert_eq!(
            counter
                .observe(true, true, &s, t + Duration::from_secs(65))
                .1["failures"],
            0
        );
    }
}
