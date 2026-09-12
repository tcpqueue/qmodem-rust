use crate::{
    at::{AtError, Next, PortPool, Program, Reply, Step},
    config::Modem,
    vendor,
};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::VecDeque, time::Duration};
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub delay_seconds: u64,
    pub commands: Vec<String>,
    pub cell_lock: Option<vendor::cells::Lock>,
    pub cell_lock_delay_seconds: u64,
    pub shutdown_reboot: bool,
    pub gpio_value_path: Option<String>,
    pub gpio_active_high: bool,
    pub sim_led: Option<String>,
    pub network_led: Option<String>,
}
impl Settings {
    pub fn validate(&self, modem: &Modem) -> Result<()> {
        ensure!(
            self.delay_seconds <= 120
                && self.cell_lock_delay_seconds <= 120
                && self.commands.len() <= 32,
            "startup delay or command count exceeds limit"
        );
        for c in &self.commands {
            Step::command(c, Duration::from_secs(30))?;
        }
        if let Some(lock) = &self.cell_lock {
            vendor::cells::lock(modem, lock)?;
        }
        if let Some(path) = &self.gpio_value_path {
            ensure!(
                path.starts_with("/sys/class/gpio/gpio")
                    && path.ends_with("/value")
                    && !path.split('/').any(|s| s == ".."),
                "GPIO value path must use /sys/class/gpio/gpioN/value"
            );
        }
        for path in [&self.sim_led, &self.network_led].into_iter().flatten() {
            ensure!(
                path.starts_with("/sys/class/leds/")
                    && path.ends_with("/brightness")
                    && !path.split('/').any(|s| s == ".."),
                "LED path must use /sys/class/leds/.../brightness"
            );
        }
        Ok(())
    }
}
struct Init {
    actions: VecDeque<Next>,
}
impl Program for Init {
    fn next(&mut self, _: &[Reply]) -> std::result::Result<Next, AtError> {
        Ok(self.actions.pop_front().unwrap_or(Next::Finish))
    }
}
fn waits(actions: &mut VecDeque<Next>, seconds: u64) {
    let mut seconds = seconds;
    while seconds > 0 {
        let n = seconds.min(5);
        actions.push_back(Next::Wait(Duration::from_secs(n)));
        seconds -= n;
    }
}
pub async fn initialize(modem: &Modem, pool: &PortPool) -> Result<Value> {
    modem.startup.validate(modem)?;
    let mut actions = VecDeque::new();
    let startup = &modem.startup;
    waits(&mut actions, startup.delay_seconds);
    let sms = &modem.sms.memories;
    actions.push_back(Next::Command(Step::command(
        &format!("AT+CPMS=\"{}\",\"{}\",\"{}\"", sms[0], sms[1], sms[2]),
        Duration::from_secs(10),
    )?));
    for c in &startup.commands {
        actions.push_back(Next::Command(Step::command(c, Duration::from_secs(30))?));
    }
    if let Some(lock) = &startup.cell_lock {
        waits(&mut actions, startup.cell_lock_delay_seconds);
        actions.push_back(Next::Command(vendor::cells::lock(modem, lock)?));
    }
    let port = pool
        .get(modem.sms_at_port.as_deref().unwrap_or(&modem.at_port))
        .await?;
    let replies = port
        .run_named(
            Box::new(Init { actions }),
            Some(modem.id.clone()),
            "post_init",
        )
        .await?;
    Ok(
        json!({"state":if replies.iter().all(|r|r.modem_success){"ready"}else{"ready_degraded"},"commands":replies.len(),"failed_commands":replies.iter().filter(|r|!r.modem_success).count()}),
    )
}
pub async fn hard_reboot(
    modem: &Modem,
    pool: &PortPool,
    runtime: &vendor::Runtime,
) -> Result<Value> {
    if let Some(path) = &modem.startup.gpio_value_path {
        modem.startup.validate(modem)?;
        let path = path.clone();
        let active = modem.startup.gpio_active_high;
        // Own the pulse to completion even if the client closes its request.
        tokio::spawn(async move {
            std::fs::write(&path, if active { b"1" } else { b"0" })?;
            tokio::time::sleep(Duration::from_secs(1)).await;
            std::fs::write(&path, if active { b"0" } else { b"1" })?;
            Ok::<_, anyhow::Error>(())
        })
        .await??;
        Ok(json!({"success":true,"method":"gpio"}))
    } else {
        let op = vendor::Operation::SoftReboot;
        let replies = pool
            .get(&modem.at_port)
            .await?
            .run_named(
                vendor::plan(modem, &op, runtime)?,
                Some(modem.id.clone()),
                "hard_reboot_fallback",
            )
            .await?;
        Ok(
            json!({"success":replies.iter().all(|r|r.modem_success),"method":"soft_reboot_fallback"}),
        )
    }
}
pub fn led(path: Option<&str>, on: bool) -> Result<()> {
    if let Some(path) = path {
        std::fs::write(path, if on { b"1" } else { b"0" })?;
    }
    Ok(())
}
pub async fn shutdown(config: crate::config::Config) {
    let pool = PortPool::default();
    let runtime = vendor::Runtime::new(config.storage.runtime_dir);
    for modem in config
        .modems
        .iter()
        .filter(|m| m.enabled && m.startup.shutdown_reboot)
    {
        for attempt in 0..3 {
            let result = async {
                let op = vendor::Operation::SoftReboot;
                let replies = pool
                    .get(&modem.at_port)
                    .await?
                    .run_named(
                        vendor::plan(modem, &op, &runtime)?,
                        Some(modem.id.clone()),
                        "shutdown_reboot",
                    )
                    .await?;
                Ok::<_, anyhow::Error>(replies.iter().all(|r| r.modem_success))
            }
            .await;
            if matches!(result, Ok(true)) {
                break;
            }
            if attempt < 2 {
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
}

pub fn signature(modem: &Modem) -> String {
    serde_json::to_string(&json!({"at":modem.at_port,"sms":modem.sms_at_port,"memories":modem.sms.memories,"startup":modem.startup})).expect("serializable startup")
}
pub fn is_ready(modem: &Modem, status: Option<&Value>) -> bool {
    status.is_some_and(|v| {
        matches!(v["state"].as_str(), Some("ready" | "ready_degraded"))
            && v["signature"] == signature(modem)
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dialing_requires_matching_completed_initialization() {
        let mut modem: Modem = serde_json::from_value(json!({"id":"m1","name":"test","manufacturer":"quectel","platform":"qualcomm","at_port":"/dev/test","bus":"usb"})).unwrap();
        assert!(!is_ready(&modem, None));
        let ready = json!({"state":"ready","signature":signature(&modem)});
        assert!(is_ready(&modem, Some(&ready)));
        modem.startup.commands.push("ATE0".into());
        assert!(!is_ready(&modem, Some(&ready)));
        assert!(!is_ready(
            &modem,
            Some(&json!({"state":"failed","signature":signature(&modem)}))
        ));
    }
}
