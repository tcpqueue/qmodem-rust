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
    pub logging: Logging,
    #[serde(default)]
    pub auth: Auth,
    #[serde(default)]
    pub modems: Vec<Modem>,
    #[serde(default)]
    pub discovery: Discovery,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Discovery {
    pub enabled: bool,
    pub interval_seconds: u64,
    pub auto_register: bool,
    pub bind_option_driver: bool,
}
impl Default for Discovery {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_seconds: 15,
            auto_register: true,
            bind_option_driver: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Server {
    pub listen: IpAddr,
    pub port: u16,
    /// Linux network device, e.g. br-lan. Empty means no device restriction.
    #[serde(default)]
    pub interface: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Logging {
    pub level: LogLevel,
    pub format: LogFormat,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Off,
    Error,
    Warn,
    #[default]
    Info,
    Debug,
    Trace,
}
impl LogLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
            Self::Trace => "trace",
        }
    }
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    #[default]
    Text,
    Json,
}
impl LogFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Json => "json",
        }
    }
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Auth {
    pub token_hash: String,
}
impl std::fmt::Debug for Auth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Auth")
            .field("configured", &!self.token_hash.is_empty())
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Storage {
    pub sqlite: String,
    #[serde(default = "default_runtime_dir")]
    pub runtime_dir: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Modem {
    pub id: String,
    pub name: String,
    #[serde(default = "enabled")]
    pub enabled: bool,
    pub manufacturer: String,
    #[serde(default)]
    pub model: String,
    pub platform: String,
    pub at_port: String,
    pub sms_at_port: Option<String>,
    pub interface: Option<String>,
    pub bus: Bus,
    #[serde(default = "default_pdp")]
    pub pdp_index: u8,
    #[serde(default)]
    pub apn: String,
    #[serde(default)]
    pub bands: crate::vendor::bands::Overrides,
    #[serde(default)]
    pub sms: crate::sms::Settings,
    #[serde(default)]
    pub network: crate::network::Settings,
    #[serde(default)]
    pub monitor: crate::monitor::Settings,
    #[serde(default)]
    pub traffic: crate::monitor::TrafficSettings,
    #[serde(default)]
    pub startup: crate::lifecycle::Settings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Bus {
    Usb,
    Pcie,
}

fn default_runtime_dir() -> String {
    "/tmp/qmodem-rust".into()
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
        ensure!(
            (5..=3600).contains(&self.discovery.interval_seconds),
            "discovery interval must be 5 to 3600 seconds"
        );
        ensure!(self.server.port != 0, "port must be between 1 and 65535");
        ensure!(
            Path::new(&self.storage.sqlite).is_absolute(),
            "SQLite path must be absolute"
        );

        ensure!(
            Path::new(&self.storage.runtime_dir).is_absolute(),
            "runtime_dir must be an absolute path on volatile storage"
        );
        for modem in &self.modems {
            modem.bands.validate()?;
            modem.sms.validate()?;
            modem.network.validate()?;
            modem.monitor.validate()?;
            modem.traffic.validate()?;
            modem.startup.validate(modem)?;
            if let Some(interface) = &modem.interface {
                validate_interface(interface)?;
            }
        }
        validate_interface(&self.server.interface)?;
        ensure!(
            self.auth.token_hash.is_empty()
                || (self.auth.token_hash.len() == 64
                    && self
                        .auth
                        .token_hash
                        .bytes()
                        .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))),
            "auth.token_hash must be a lowercase SHA-256 hex digest"
        );
        let mut ids = HashSet::new();
        let mut logical_names = HashSet::new();
        for modem in &self.modems {
            crate::vendor::family(modem)?;
            let logical = crate::network::interface_name(modem);
            ensure!(
                logical_names.insert(logical.clone())
                    && logical_names.insert(format!("{logical}v6")),
                "duplicate logical network interface"
            );
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
            Config::parse(&SAMPLE.replace("/etc/qmodem-rust/data.sqlite3", "data.sqlite3"))
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

pub fn validate_interface(name: &str) -> Result<()> {
    ensure!(
        name.is_empty()
            || (name.len() < 16
                && name != "."
                && name != ".."
                && name
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.'))),
        "invalid network device name (maximum 15 ASCII characters)"
    );
    Ok(())
}

/// A sidecar lock serializes writers. Atomic rename protects readers; comments and
/// fields outside this patch stay intact. Callers must never build TOML from shell text.
pub fn update(
    path: &Path,
    patch: impl FnOnce(&mut toml_edit::DocumentMut) -> Result<()>,
) -> Result<()> {
    use fs2::FileExt;
    use std::{fs::OpenOptions, io::Write, os::unix::fs::OpenOptionsExt};
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path.with_extension("toml.lock"))?;
    lock.lock_exclusive()?;
    let original = fs::read_to_string(path)?;
    let mut document = original.parse::<toml_edit::DocumentMut>()?;
    patch(&mut document)?;
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

#[derive(Debug, Default)]
pub struct ServicePatch {
    pub listen: Option<IpAddr>,
    pub port: Option<u16>,
    pub interface: Option<String>,
    pub log_level: Option<LogLevel>,
    pub log_format: Option<LogFormat>,
}
pub fn set_service(path: &Path, patch: ServicePatch) -> Result<()> {
    update(path, |d| {
        if let Some(listen) = patch.listen {
            d["server"]["listen"] = toml_edit::value(listen.to_string());
        }
        if let Some(port) = patch.port {
            d["server"]["port"] = toml_edit::value(i64::from(port));
        }
        if let Some(interface) = patch.interface {
            d["server"]["interface"] = toml_edit::value(interface);
        }
        if let Some(level) = patch.log_level {
            d["logging"]["level"] = toml_edit::value(level.as_str());
        }
        if let Some(format) = patch.log_format {
            d["logging"]["format"] = toml_edit::value(format.as_str());
        }
        Ok(())
    })
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
        set_service(
            &path,
            ServicePatch {
                listen: Some("::1".parse().unwrap()),
                port: Some(9999),
                ..Default::default()
            },
        )
        .unwrap();
        let updated = fs::read_to_string(&path).unwrap();
        assert!(updated.contains("# [[modems]]"));
        let config = Config::load(&path).unwrap();
        assert_eq!(config.server.port, 9999);
        assert!(config.server.listen.is_ipv6());
        assert!(
            set_service(
                &path,
                ServicePatch {
                    port: Some(0),
                    ..Default::default()
                }
            )
            .is_err()
        );
        assert_eq!(fs::read_to_string(path).unwrap(), updated);
    }
}

#[cfg(test)]
mod settings_tests {
    use super::*;
    #[test]
    fn interface_names_and_levels_are_strict() {
        for name in ["br-lan", "eth0", "eth0.10", "lo", ""] {
            validate_interface(name).unwrap();
        }
        for name in [
            "../eth0",
            "eth0\n",
            "eth0;reboot",
            "eth0 lo",
            "0123456789012345",
        ] {
            assert!(validate_interface(name).is_err());
        }
        let sample = include_str!("../../../config/qmodem.example.toml");
        assert!(Config::parse(&sample.replace("level = \"info\"", "level = \"verbose\"")).is_err());
    }
    #[test]
    fn updates_keep_auth_and_comments_and_old_config_gets_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let original = format!(
            "version=1\n[server]\nlisten='127.0.0.1'\nport=8088\n[storage]\nsqlite='/tmp/test.sqlite3'\n# preserve this\n[auth]\ntoken_hash='{}'\n",
            "a".repeat(64)
        );
        fs::write(&path, &original).unwrap();
        assert_eq!(Config::load(&path).unwrap().logging.level, LogLevel::Info);
        set_service(
            &path,
            ServicePatch {
                interface: Some("br-lan".into()),
                log_level: Some(LogLevel::Trace),
                log_format: Some(LogFormat::Json),
                ..Default::default()
            },
        )
        .unwrap();
        let cfg = Config::load(&path).unwrap();
        assert_eq!(cfg.logging.level, LogLevel::Trace);
        assert_eq!(cfg.server.interface, "br-lan");
        assert_eq!(cfg.auth.token_hash, "a".repeat(64));
        assert!(
            fs::read_to_string(&path)
                .unwrap()
                .contains("# preserve this")
        );
        let before = fs::read_to_string(&path).unwrap();
        assert!(
            set_service(
                &path,
                ServicePatch {
                    interface: Some("../bad".into()),
                    ..Default::default()
                }
            )
            .is_err()
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), before);
        set_service(
            &path,
            ServicePatch {
                interface: Some(String::new()),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(Config::load(&path).unwrap().server.interface.is_empty());
    }
}
