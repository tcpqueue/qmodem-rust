use crate::{at::PortPool, config::Modem, vendor};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::PathBuf;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ResetSchedule {
    pub enabled: bool,
    pub kind: Kind,
    pub hour: u8,
    pub day: u8,
}
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Daily,
    Weekly,
    Monthly,
}
impl ResetSchedule {
    pub fn validate(&self) -> Result<()> {
        ensure!(self.hour < 24, "reset hour must be 0 to 23");
        if self.enabled {
            ensure!(
                match self.kind {
                    Kind::Daily => true,
                    Kind::Weekly => self.day <= 6,
                    Kind::Monthly => (1..=31).contains(&self.day),
                },
                "invalid reset day"
            );
        }
        Ok(())
    }
    fn key(&self, time: &nix::libc::tm) -> Option<String> {
        if !self.enabled || time.tm_hour != i32::from(self.hour) {
            return None;
        }
        let day = match self.kind {
            Kind::Daily => true,
            Kind::Weekly => time.tm_wday == i32::from(self.day),
            Kind::Monthly => time.tm_mday == i32::from(self.day),
        };
        day.then(|| {
            format!(
                "{}-{}-{}",
                time.tm_year + 1900,
                time.tm_mon + 1,
                time.tm_mday
            )
        })
    }
}
fn claim(db: &rusqlite::Connection, modem: &str, period: &str) -> Result<bool> {
    Ok(db.execute("INSERT OR IGNORE INTO maintenance_runs(modem_id,action,period,state) VALUES (?,'traffic_reset',?,'started')",rusqlite::params![modem,period])?==1)
}
pub async fn tick(
    modem: Modem,
    pool: PortPool,
    path: PathBuf,
    runtime: vendor::Runtime,
) -> Result<Option<Value>> {
    let schedule = &modem.traffic.reset;
    if !schedule.enabled {
        return Ok(None);
    }
    // The 32-bit musl ABI still exposes a narrower time_t.
    #[allow(clippy::useless_conversion)]
    let epoch = crate::sms::database::now()
        .try_into()
        .map_err(|_| anyhow::anyhow!("system time exceeds the platform calendar range"))?;
    // localtime_r writes only into the supplied tm. libc uses the router timezone.
    let period = {
        let mut local = std::mem::MaybeUninit::<nix::libc::tm>::uninit();
        if unsafe { nix::libc::localtime_r(&epoch, local.as_mut_ptr()) }.is_null() {
            return Ok(None);
        }
        let local = unsafe { local.assume_init() };
        schedule.key(&local)
    };
    let Some(period) = period else {
        return Ok(None);
    };
    tokio::spawn(async move{
        let id=modem.id.clone();let key=period.clone();
        if !crate::sms::database::run(path.clone(),move|db|claim(db,&id,&key)).await?{return Ok(None)}
        let outcome=async{
            let op=vendor::Operation::ClearUsageStats;
            if vendor::local(&modem,&op,&runtime)?.is_some(){anyhow::bail!("modem counter reset is unsupported")}
            let replies=pool.get(&modem.at_port).await?.run_named(vendor::plan(&modem,&op,&runtime)?,Some(modem.id.clone()),"scheduled_traffic_reset").await?;
            ensure!(replies.iter().all(|r|r.modem_success),"modem rejected counter reset");
            Ok::<_,anyhow::Error>(())
        }.await;
        let success=outcome.is_ok();
        let id=modem.id.clone();
        crate::sms::database::run(path,move|db|{
            db.execute("UPDATE maintenance_runs SET state=? WHERE modem_id=? AND action='traffic_reset' AND period=?",rusqlite::params![if success{"completed"}else{"failed"},id,period])?;
            Ok(())
        }).await?;
        Ok::<_,anyhow::Error>(Some(json!({"success":success,"attempted_at":epoch,"error":outcome.err().map(|e|e.to_string())})))
    }).await?
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn schedule_matches_router_calendar_and_survives_restart() {
        let mut local: nix::libc::tm = unsafe { std::mem::zeroed() };
        local.tm_year = 126;
        local.tm_mon = 8;
        local.tm_mday = 13;
        local.tm_wday = 0;
        local.tm_hour = 3;
        let mut schedule = ResetSchedule {
            enabled: true,
            kind: Kind::Weekly,
            hour: 3,
            day: 0,
        };
        let period = schedule.key(&local).unwrap();
        assert_eq!(period, "2026-9-13");
        local.tm_hour = 4;
        assert!(schedule.key(&local).is_none());
        local.tm_hour = 3;
        schedule.kind = Kind::Monthly;
        schedule.day = 31;
        assert!(schedule.key(&local).is_none());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db");
        let db = crate::storage::initialize(&path).unwrap();
        assert!(claim(&db, "m", &period).unwrap());
        drop(db);
        let db = crate::storage::initialize(&path).unwrap();
        assert!(!claim(&db, "m", &period).unwrap());
        assert!(claim(&db, "other", &period).unwrap());
    }
}
