use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, fs, net::IpAddr, path::Path};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub version: u32,
    pub server: Server,
    pub storage: Storage,
    #[serde(default)]
    pub modems: Vec<Modem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Server {
    pub listen: IpAddr,
    pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Storage {
    pub sqlite: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Modem {
    pub id: String,
    pub name: String,
    #[serde(default = "enabled")]
    pub enabled: bool,
    pub manufacturer: String,
    pub platform: String,
    pub at_port: String,
    pub sms_at_port: Option<String>,
    pub interface: Option<String>,
    pub bus: Bus,
    #[serde(default = "default_pdp")]
    pub pdp_index: u8,
    #[serde(default)]
    pub apn: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Bus {
    Usb,
    Pcie,
}

fn enabled() -> bool {
    true
}
fn default_pdp() -> u8 {
    1
}

impl Config {
    pub fn parse(text: &str) -> Result<Self> {
        let config: Self = toml::from_str(text).context("invalid TOML configuration")?;
        config.validate()?;
        Ok(config)
    }

    pub fn load(path: &Path) -> Result<Self> {
        Self::parse(&fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.version == 1,
            "unsupported configuration version: {}",
            self.version
        );
        ensure!(self.server.port != 0, "port must be between 1 and 65535");
        ensure!(
            Path::new(&self.storage.sqlite).is_absolute(),
            "SQLite path must be absolute"
        );
        let mut ids = HashSet::new();
        for modem in &self.modems {
            ensure!(
                !modem.id.is_empty()
                    && modem.id.len() <= 64
                    && modem
                        .id
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-'),
                "invalid modem id"
            );
            ensure!(ids.insert(&modem.id), "duplicate modem id: {}", modem.id);
            ensure!(
                (1..=16).contains(&modem.pdp_index),
                "PDP index must be between 1 and 16"
            );
            ensure!(
                modem.at_port.starts_with("/dev/") && !modem.at_port.split('/').any(|s| s == ".."),
                "AT port must be under /dev"
            );
            if let Some(port) = &modem.sms_at_port {
                ensure!(
                    port.starts_with("/dev/") && !port.split('/').any(|s| s == ".."),
                    "SMS port must be under /dev"
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const SAMPLE: &str = include_str!("../../../config/qmodem.example.toml");

    #[test]
    fn example_is_valid() {
        let cfg = Config::parse(SAMPLE).unwrap();
        assert_eq!(cfg.server.port, 8088);
        assert!(cfg.modems.is_empty());
    }

    #[test]
    fn bad_port_and_unknown_keys_are_rejected() {
        for text in [
            SAMPLE.replace("8088", "0"),
            SAMPLE.replace("8088", "65536"),
            SAMPLE.replace("port =", "prot ="),
        ] {
            assert!(Config::parse(&text).is_err());
        }
    }

    #[test]
    fn version_and_storage_are_validated() {
        assert!(Config::parse(&SAMPLE.replace("version = 1", "version = 2")).is_err());
        assert!(
            Config::parse(&SAMPLE.replace("/var/lib/qmodem-rust/qmodem.sqlite3", "data.sqlite3"))
                .is_err()
        );
    }

    #[test]
    fn ipv6_is_accepted() {
        assert!(
            Config::parse(&SAMPLE.replace("127.0.0.1", "::1"))
                .unwrap()
                .server
                .listen
                .is_ipv6()
        );
    }

    #[test]
    fn duplicate_modems_and_path_traversal_are_rejected() {
        let block = "\n[[modems]]\nid='modem_1'\nname='test'\nmanufacturer='quectel'\nplatform='qualcomm'\nat_port='/dev/ttyUSB2'\nbus='usb'\n";
        assert!(Config::parse(&format!("{SAMPLE}{block}")).is_ok());
        assert!(Config::parse(&format!("{SAMPLE}{block}{block}")).is_err());
        assert!(
            Config::parse(
                &format!("{SAMPLE}{block}").replace("/dev/ttyUSB2", "/dev/../etc/passwd")
            )
            .is_err()
        );
    }
}

/// Update the service fields without rewriting unrelated values or comments.
/// A sidecar lock serializes CLI writers; rename makes reader snapshots atomic.
pub fn set_service(path: &Path, listen: IpAddr, port: u16) -> Result<()> {
    use fs2::FileExt;
    use std::{fs::OpenOptions, io::Write, os::unix::fs::OpenOptionsExt};
    let lock_path = path.with_extension("toml.lock");
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(lock_path)?;
    lock.lock_exclusive()?;
    let original = fs::read_to_string(path)?;
    let mut document = original.parse::<toml_edit::DocumentMut>()?;
    document["server"]["listen"] = toml_edit::value(listen.to_string());
    document["server"]["port"] = toml_edit::value(i64::from(port));
    let updated = document.to_string();
    Config::parse(&updated)?;
    let directory = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
    temporary
        .as_file()
        .set_permissions(fs::metadata(path)?.permissions())?;
    temporary.write_all(updated.as_bytes())?;
    temporary.as_file().sync_all()?;
    temporary.persist(path)?;
    fs::File::open(directory)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod update_tests {
    use super::*;
    #[test]
    fn atomic_update_retains_comments_and_rejects_invalid_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("qmodem.toml");
        let original = include_str!("../../../config/qmodem.example.toml");
        fs::write(&path, original).unwrap();
        set_service(&path, "::1".parse().unwrap(), 9999).unwrap();
        let updated = fs::read_to_string(&path).unwrap();
        assert!(updated.contains("# [[modems]]"));
        let config = Config::load(&path).unwrap();
        assert_eq!(config.server.port, 9999);
        assert!(config.server.listen.is_ipv6());
        assert!(set_service(&path, "127.0.0.1".parse().unwrap(), 0).is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), updated);
    }
}
