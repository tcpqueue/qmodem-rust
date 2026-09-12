// SPDX-License-Identifier: GPL-3.0-only
// Legacy field mapping follows qmodem_smsd/legacy_migrate.c.
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, params};
use serde_json::{Value, json};

pub fn import(db: &mut Connection, modem: &str, source: &str, document: &Value) -> Result<Value> {
    ensure!(
        !source.is_empty() && source.len() <= 256,
        "source must be 1 to 256 bytes"
    );
    ensure!(document.is_object(), "legacy history must be a JSON object");
    let tx = db.transaction()?;
    let mut imported = 0;
    let mut skipped = 0;
    let mut present = false;
    for direction in ["sent", "received"] {
        let Some(entries) = document.get(direction) else {
            continue;
        };
        present = true;
        let entries = entries
            .as_array()
            .context("history entry must be an array")?;
        ensure!(entries.len() <= 10000, "at most 10000 records per import");
        for entry in entries {
            let id = entry["id"]
                .as_i64()
                .filter(|id| *id >= 0)
                .context("invalid legacy id")?;
            let timestamp = entry["timestamp"]
                .as_i64()
                .filter(|t| *t >= 0)
                .context("invalid legacy timestamp")?;
            let content = entry["content"]
                .as_str()
                .context("missing legacy content")?;
            let peer = entry[if direction == "sent" {
                "recipient"
            } else {
                "sender"
            }]
            .as_str()
            .context("missing legacy peer")?;
            ensure!(
                content.len() <= 32768 && peer.len() <= 128,
                "legacy record exceeds limits"
            );
            let key = format!("{direction}:{id}");
            let fresh=tx.execute("INSERT OR IGNORE INTO legacy_imports(modem_id,source,record_key,imported_at) VALUES (?,?,?,?)",params![modem,source,key,super::database::now()])?;
            if fresh == 0 {
                skipped += 1;
                continue;
            }
            let flag = |field: &str| {
                entry[field]
                    .as_bool()
                    .unwrap_or_else(|| entry[field].as_i64().is_some_and(|n| n != 0))
            };
            let status = if direction == "received" {
                "received"
            } else if flag("is_success") {
                "submitted"
            } else {
                "failed"
            };
            tx.execute("INSERT INTO messages(modem_id,direction,peer,content,timestamp,is_read,delivery_status,metadata) VALUES (?,?,?,?,?,?,?,?)",
                params![modem,direction,peer,content,timestamp,flag("is_read"),status,json!({"legacy":true,"source":source,"legacy_id":id}).to_string()])?;
            imported += 1;
        }
    }
    ensure!(present, "JSON must contain sent or received arrays");
    tx.commit()?;
    Ok(json!({"imported":imported,"skipped":skipped}))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_import_preserves_read_state_and_is_atomic_and_repeatable() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = crate::storage::initialize(&dir.path().join("db")).unwrap();
        let valid = json!({"received":[{"id":1,"timestamp":123,"sender":"10086","content":"余额","is_read":1}]});
        assert_eq!(
            import(&mut db, "m", "old_received.json", &valid).unwrap()["imported"],
            1
        );
        assert_eq!(
            import(&mut db, "m", "old_received.json", &valid).unwrap()["skipped"],
            1
        );
        let bad = json!({"sent":[{"id":2,"timestamp":123,"recipient":"10086","content":"hi","is_success":1},{"id":3}]});
        assert!(import(&mut db, "m", "old_sent.json", &bad).is_err());
        let entries = super::super::database::list(&db, "m", None, None, 50).unwrap();
        assert_eq!(entries["items"].as_array().unwrap().len(), 1);
        assert_eq!(entries["items"][0]["is_read"], true);
        assert_eq!(entries["items"][0]["metadata"]["legacy"], true);
    }
}
