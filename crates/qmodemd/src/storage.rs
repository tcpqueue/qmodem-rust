use anyhow::{Result, ensure};
use rusqlite::Connection;
use std::{fs, path::Path, time::Duration};

pub fn initialize(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut db = Connection::open(path)?;
    db.busy_timeout(Duration::from_secs(5))?;
    db.pragma_update(None, "journal_mode", "WAL")?;
    db.pragma_update(None, "foreign_keys", "ON")?;
    let version: u32 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
    ensure!(version <= 5, "database was created by a newer version");
    if version == 0 {
        let tx = db.transaction()?;
        tx.execute_batch(
            "CREATE TABLE messages (
                id INTEGER PRIMARY KEY,
                modem_id TEXT NOT NULL,
                direction TEXT NOT NULL CHECK(direction IN ('received', 'sent')),
                peer TEXT NOT NULL,
                content TEXT NOT NULL,
                timestamp INTEGER NOT NULL,
                is_read INTEGER NOT NULL DEFAULT 0 CHECK(is_read IN (0, 1)),
                delivery_status TEXT NOT NULL DEFAULT 'unknown',
                sim_index INTEGER,
                pdu TEXT
            );
            CREATE INDEX messages_modem_time ON messages(modem_id, timestamp DESC, id DESC);
            CREATE TABLE traffic (
                modem_id TEXT NOT NULL,
                timestamp INTEGER NOT NULL,
                rx_bytes INTEGER NOT NULL CHECK(rx_bytes >= 0),
                tx_bytes INTEGER NOT NULL CHECK(tx_bytes >= 0),
                PRIMARY KEY(modem_id, timestamp)
            );
            PRAGMA user_version = 1;",
        )?;
        tx.commit()?;
    }
    if version <= 1 {
        let tx = db.transaction()?;
        tx.execute_batch("ALTER TABLE messages ADD COLUMN metadata TEXT NOT NULL DEFAULT '{}';
            ALTER TABLE messages ADD COLUMN request_id TEXT;
            CREATE UNIQUE INDEX messages_request ON messages(modem_id,request_id) WHERE request_id IS NOT NULL;
            CREATE TABLE sms_segments (id INTEGER PRIMARY KEY,modem_id TEXT NOT NULL,message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,reference INTEGER,total INTEGER NOT NULL,part INTEGER NOT NULL,pdu TEXT NOT NULL,content TEXT NOT NULL,sim_index INTEGER,UNIQUE(modem_id,pdu),UNIQUE(message_id,part));
            CREATE INDEX sms_segments_group ON sms_segments(modem_id,reference,total);
            PRAGMA user_version=2;")?;
        tx.commit()?;
    }
    if version <= 2 {
        let tx = db.transaction()?;
        tx.execute_batch("CREATE TABLE sms_forward_start(modem_id TEXT NOT NULL,sink_id TEXT NOT NULL,minimum_id INTEGER NOT NULL,PRIMARY KEY(modem_id,sink_id));
        CREATE TABLE sms_deliveries(id INTEGER PRIMARY KEY,message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,modem_id TEXT NOT NULL,sink_id TEXT NOT NULL,state TEXT NOT NULL,attempts INTEGER NOT NULL DEFAULT 0,available_at INTEGER NOT NULL,claim_token TEXT,last_error TEXT,UNIQUE(message_id,sink_id));
        CREATE INDEX sms_deliveries_due ON sms_deliveries(state,available_at);
        PRAGMA user_version=3;")?;
        tx.commit()?;
    }
    if version <= 3 {
        let tx = db.transaction()?;
        tx.execute_batch("ALTER TABLE traffic ADD COLUMN source TEXT NOT NULL DEFAULT 'unknown';
        ALTER TABLE sms_forward_start ADD COLUMN fingerprint TEXT NOT NULL DEFAULT '';
        CREATE TABLE legacy_imports(modem_id TEXT NOT NULL, source TEXT NOT NULL, record_key TEXT NOT NULL, imported_at INTEGER NOT NULL, PRIMARY KEY(modem_id,source,record_key));
        PRAGMA user_version=4;")?;
        tx.commit()?;
    }
    if version <= 4 {
        db.execute_batch("CREATE TABLE maintenance_runs(modem_id TEXT NOT NULL, action TEXT NOT NULL,period TEXT NOT NULL,state TEXT NOT NULL,PRIMARY KEY(modem_id,action,period)); PRAGMA user_version=5;")?;
    }
    Ok(db)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_is_idempotent_and_preserves_history() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data/qmodem.sqlite3");
        let db = initialize(&path).unwrap();
        db.execute("INSERT INTO messages(modem_id,direction,peer,content,timestamp) VALUES ('m1','received','10086','测试',1)", []).unwrap();
        drop(db);
        let db = initialize(&path).unwrap();
        let content: String = db
            .query_row("SELECT content FROM messages", [], |r| r.get(0))
            .unwrap();
        assert_eq!(content, "测试");
        assert!(
            db.execute(
                "INSERT INTO traffic(modem_id,timestamp,rx_bytes,tx_bytes) VALUES ('m1',1,-1,0)",
                []
            )
            .is_err()
        );
    }

    #[test]
    fn newer_schema_is_not_downgraded() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("q.sqlite3");
        let db = Connection::open(&path).unwrap();
        db.pragma_update(None, "user_version", 6).unwrap();
        drop(db);
        assert!(initialize(&path).is_err());
    }
}
