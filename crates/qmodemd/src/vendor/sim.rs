// SPDX-License-Identifier: GPL-3.0-only
// AT behavior ported from FUjr/QModem vendor/quectel.sh and vendor/huawei.sh.
// Copyright (C) 2023 Siriling <siriling@qq.com>
// Copyright (C) 2025 Fujr <fjrcn@outlook.com>
// Copyright (C) 2025 coolsnowwolf <coolsnowwolf@gmail.com>
// Rust adaptation Copyright (C) 2026 tcpqueue
use super::*;
use crate::at::{AtError, ErrorKind, Next, Program};
use std::{
    fs,
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

/// Software state lives on volatile storage, as upstream's /tmp file did.
/// A daemon restart retains it; rebooting the router clears it.
#[derive(Clone)]
pub struct Runtime {
    root: PathBuf,
}
impl Runtime {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
    fn path(&self, id: &str) -> PathBuf {
        // Encode IDs instead of letting modem names become filesystem paths.
        self.root.join(format!(
            "sim-{}",
            id.as_bytes()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        ))
    }
    fn directory(&self) -> Result<()> {
        match fs::DirBuilder::new().mode(0o700).create(&self.root) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
        let meta = fs::symlink_metadata(&self.root)?;
        ensure!(
            meta.is_dir()
                && meta.uid() == nix::unistd::geteuid().as_raw()
                && meta.mode() & 0o077 == 0,
            "runtime directory must be owned by the service user, private (0700), and not a symlink"
        );
        Ok(())
    }
    pub fn slot(&self, id: &str) -> Result<u8> {
        self.directory()?;
        let file = match fs::OpenOptions::new()
            .read(true)
            .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK)
            .open(self.path(id))
        {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(e) => return Err(e.into()),
        };
        ensure!(
            file.metadata()?.is_file(),
            "SIM state must be a regular file"
        );
        let mut value = String::new();
        file.take(4).read_to_string(&mut value)?;
        match value.as_str() {
            "0\n" => Ok(0),
            "1\n" => Ok(1),
            _ => bail!("invalid software SIM state"),
        }
    }
    pub fn set(&self, id: &str, slot: u8) -> Result<()> {
        ensure!(slot <= 1, "invalid MT5700 slot");
        self.directory()?;
        let mut file = tempfile::NamedTempFile::new_in(&self.root)?;
        writeln!(file, "{slot}")?;
        file.as_file().sync_all()?;
        file.persist(self.path(id))?;
        fs::File::open(Path::new(&self.root))?.sync_all()?;
        Ok(())
    }
}
fn value(line: &str) -> Option<&str> {
    // awk matches anywhere on a line, then takes the second colon-separated field.
    if !line.contains("+QUIMSLOT:") && !line.contains("+QUSIMSLOT:") {
        return None;
    }
    line.split(':').nth(1)
}
pub(super) fn parse_slot(raw: &str) -> Option<u8> {
    raw.lines().filter_map(value).find_map(|v| {
        let digits: String = v.chars().filter(char::is_ascii_digit).collect();
        match digits.as_str() {
            "1" => Some(1),
            "2" => Some(2),
            _ => None,
        }
    })
}
pub(super) fn capabilities(raw: &str) -> Value {
    let slots = raw
        .lines()
        .find_map(value)
        .unwrap_or("")
        .replace(['(', ')', ','], " ");
    let values: Vec<u8> = slots
        .split_whitespace()
        .filter_map(|s| match s {
            "1" => Some(1),
            "2" => Some(2),
            _ => None,
        })
        .collect();
    json!({"supported":!slots.is_empty(),"slots":values,"source":"modem"})
}
pub(super) struct Switch {
    pub slot: u8,
    pub sent: bool,
    pub slept_after: usize,
}
impl Program for Switch {
    fn next(&mut self, replies: &[Reply]) -> std::result::Result<Next, AtError> {
        let command = |c: &str| {
            Next::Command(Step::command(c, Duration::from_secs(10)).expect("validated SIM command"))
        };
        if !self.sent {
            self.sent = true;
            return Ok(command(&format!("AT+QUIMSLOT={}", self.slot)));
        }
        let Some(first) = replies.first() else {
            return Ok(Next::Finish);
        };
        if !first.response.lines().any(|l| l.starts_with("OK")) {
            return Ok(Next::Finish);
        }
        let polls = replies.len() - 1;
        if polls > 0 {
            if parse_slot(&replies.last().unwrap().response) == Some(self.slot) {
                return Ok(Next::Finish);
            }
            if self.slept_after < polls {
                self.slept_after = polls;
                return Ok(Next::Wait(Duration::from_secs(1)));
            }
        }
        if polls >= 5 {
            Ok(Next::Finish)
        } else {
            Ok(command("AT+QUIMSLOT?"))
        }
    }
}
pub(super) struct SoftwareSwitch {
    pub runtime: Runtime,
    pub id: String,
    pub slot: u8,
    pub command: Option<Step>,
}
impl Program for SoftwareSwitch {
    fn next(&mut self, _replies: &[Reply]) -> std::result::Result<Next, AtError> {
        let Some(command) = self.command.take() else {
            return Ok(Next::Finish);
        };
        // Preserve upstream write-before-AT behavior, even when the modem rejects.
        self.runtime.set(&self.id, self.slot).map_err(|_| AtError {
            kind: ErrorKind::State,
            message: "Could not save software SIM state".into(),
        })?;
        Ok(Next::Command(command))
    }
}
