use super::pdu::Decoded;
use anyhow::{Result, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
pub async fn run<T: Send + 'static>(
    path: PathBuf,
    operation: impl FnOnce(&mut Connection) -> Result<T> + Send + 'static,
) -> Result<T> {
    tokio::task::spawn_blocking(move || {
        let mut db = crate::storage::initialize(&path)?;
        operation(&mut db)
    })
    .await?
}
fn row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    let metadata: String = row.get(9)?;
    Ok(
        json!({"id":row.get::<_,i64>(0)?,"modem_id":row.get::<_,String>(1)?,"direction":row.get::<_,String>(2)?,"peer":row.get::<_,String>(3)?,"content":row.get::<_,String>(4)?,"timestamp":row.get::<_,i64>(5)?,"is_read":row.get::<_,bool>(6)?,"delivery_status":row.get::<_,String>(7)?,"sim_index":row.get::<_,Option<i64>>(8)?,"metadata":serde_json::from_str::<Value>(&metadata).unwrap_or(Value::Null)}),
    )
}
const SELECT: &str = "SELECT id,modem_id,direction,peer,content,timestamp,is_read,delivery_status,sim_index,metadata FROM messages";
pub fn list(
    db: &Connection,
    modem: &str,
    peer: Option<&str>,
    before: Option<i64>,
    limit: u32,
) -> Result<Value> {
    ensure!((1..=200).contains(&limit), "page limit must be 1 to 200");
    let mut stmt=db.prepare(&format!("{SELECT} WHERE modem_id=?1 AND (?2 IS NULL OR peer=?2) AND (?3 IS NULL OR id<?3) ORDER BY id DESC LIMIT ?4"))?;
    let items = stmt
        .query_map(params![modem, peer, before, limit], row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let cursor = if items.len() == limit as usize {
        items.last().and_then(|m| m["id"].as_i64())
    } else {
        None
    };
    Ok(json!({"items":items,"next_cursor":cursor}))
}
pub fn get(db: &Connection, modem: &str, id: i64) -> Result<Option<Value>> {
    Ok(db
        .query_row(
            &format!("{SELECT} WHERE modem_id=? AND id=?"),
            params![modem, id],
            row,
        )
        .optional()?)
}
pub fn conversations(db: &Connection, modem: &str) -> Result<Value> {
    let mut stmt=db.prepare("SELECT peer,COUNT(*),SUM(CASE WHEN is_read=0 AND direction='received' THEN 1 ELSE 0 END),MAX(timestamp),MAX(id) FROM messages WHERE modem_id=? GROUP BY peer ORDER BY MAX(id) DESC LIMIT 500")?;
    let rows=stmt.query_map([modem],|r|Ok(json!({"peer":r.get::<_,String>(0)?,"count":r.get::<_,i64>(1)?,"unread":r.get::<_,i64>(2)?,"timestamp":r.get::<_,i64>(3)?,"latest_id":r.get::<_,i64>(4)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(json!({"items":rows}))
}
pub fn import(
    db: &mut Connection,
    modem: &str,
    index: i64,
    pdu: &str,
    decoded: &Decoded,
) -> Result<bool> {
    let tx = db.transaction()?;
    let known: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sms_segments WHERE modem_id=? AND pdu=?)",
        params![modem, pdu],
        |r| r.get(0),
    )?;
    if known {
        return Ok(false);
    }
    let timestamp = decoded.timestamp.unwrap_or_else(now);
    let mut message_id = None;
    if let Some(c) = &decoded.concat {
        // A repeated reference may denote a new message. Join only an open group in the
        // same time window whose part number has not already been occupied.
        message_id=tx.query_row("SELECT s.message_id FROM sms_segments s JOIN messages m ON m.id=s.message_id WHERE s.modem_id=?1 AND m.peer=?2 AND s.reference=?3 AND s.total=?4 AND ABS(m.timestamp-?5)<=86400 AND m.delivery_status='incomplete' AND NOT EXISTS(SELECT 1 FROM sms_segments p WHERE p.message_id=s.message_id AND p.part=?6) ORDER BY m.id DESC LIMIT 1",params![modem,decoded.peer,c.reference,c.total,timestamp,c.part],|r|r.get::<_,i64>(0)).optional()?;
    }
    let id = match message_id {
        Some(id) => id,
        None => {
            tx.execute("INSERT INTO messages(modem_id,direction,peer,content,timestamp,is_read,delivery_status,sim_index,pdu,metadata) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",params![modem,decoded.direction,decoded.peer,decoded.content,timestamp,decoded.direction=="sent",if decoded.concat.is_some(){"incomplete"}else{"received"},index,pdu,serde_json::to_string(decoded)?])?;
            tx.last_insert_rowid()
        }
    };
    tx.execute("INSERT INTO sms_segments(modem_id,message_id,reference,total,part,pdu,content,sim_index) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",params![modem,id,decoded.concat.as_ref().map(|c|c.reference),decoded.concat.as_ref().map_or(1,|c|c.total),decoded.concat.as_ref().map_or(1,|c|c.part),pdu,decoded.content,index])?;
    if let Some(c) = &decoded.concat {
        let mut stmt =
            tx.prepare("SELECT part,content FROM sms_segments WHERE message_id=? ORDER BY part")?;
        let segments = stmt
            .query_map([id], |r| Ok((r.get::<_, u8>(0)?, r.get::<_, String>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let complete = segments.len() == c.total as usize;
        let content = segments
            .iter()
            .map(|(_, text)| text.as_str())
            .collect::<String>();
        tx.execute("UPDATE messages SET content=?,delivery_status=?,metadata=? WHERE id=?",params![content,if complete{"received"}else{"incomplete"},json!({"encoding":decoded.encoding,"parts_received":segments.iter().map(|(i,_)|i).collect::<Vec<_>>(),"total_parts":c.total,"reference":c.reference}).to_string(),id])?;
    }
    tx.commit()?;
    Ok(true)
}
pub fn begin_send(
    db: &Connection,
    modem: &str,
    request: &str,
    peer: &str,
    text: &str,
) -> Result<(i64, bool)> {
    let inserted=db.execute("INSERT OR IGNORE INTO messages(modem_id,direction,peer,content,timestamp,is_read,delivery_status,request_id) VALUES (?,'sent',?,?,?,1,'sending',?)",params![modem,peer,text,now(),request])?;
    let (id, previous_peer, previous_text): (i64, String, String) = db.query_row(
        "SELECT id,peer,content FROM messages WHERE modem_id=? AND request_id=?",
        params![modem, request],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    ensure!(
        previous_peer == peer && previous_text == text,
        "request_id has already been used with different SMS content"
    );
    Ok((id, inserted == 1))
}

pub fn finish_send(db: &Connection, id: i64, status: &str, details: &Value) -> Result<()> {
    db.execute(
        "UPDATE messages SET delivery_status=?,metadata=? WHERE id=?",
        params![status, details.to_string(), id],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sms::pdu;
    #[test]
    fn multipart_import_is_durable_ordered_and_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sms.sqlite3");
        let mut db = crate::storage::initialize(&path).unwrap();
        let text = "短信😀".repeat(50);
        let parts = pdu::encode("10086", &text, 9).unwrap();
        for (i, part) in parts.iter().enumerate().rev() {
            let mut decoded = pdu::decode(&part.pdu).unwrap();
            decoded.direction = "received";
            decoded.timestamp = Some(1770000000);
            assert!(import(&mut db, "m1", i as i64, &part.pdu, &decoded).unwrap());
            assert!(!import(&mut db, "m1", i as i64, &part.pdu, &decoded).unwrap());
        }
        let list = list(&db, "m1", None, None, 50).unwrap();
        assert_eq!(list["items"].as_array().unwrap().len(), 1);
        assert_eq!(list["items"][0]["content"], text);
        assert_eq!(list["items"][0]["delivery_status"], "received");
        drop(db);
        let db = crate::storage::initialize(&path).unwrap();
        assert_eq!(conversations(&db, "m1").unwrap()["items"][0]["unread"], 1);
    }
    #[test]
    fn duplicate_send_key_never_creates_a_second_outgoing_message() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::storage::initialize(&dir.path().join("sms.sqlite3")).unwrap();
        let (id, fresh) = begin_send(&db, "m1", "req-1", "10086", "hi").unwrap();
        assert!(fresh);
        assert_eq!(
            begin_send(&db, "m1", "req-1", "10086", "hi").unwrap(),
            (id, false)
        );
        assert!(begin_send(&db, "m1", "req-1", "10086", "different").is_err());
        finish_send(&db, id, "submitted", &json!({"references":[7]})).unwrap();
        assert_eq!(
            get(&db, "m1", id).unwrap().unwrap()["delivery_status"],
            "submitted"
        );
    }
    #[test]
    fn reference_reuse_and_duplicate_part_do_not_mix_messages() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = crate::storage::initialize(&dir.path().join("sms.sqlite3")).unwrap();
        for text in ["A".repeat(200), "B".repeat(200)] {
            let parts = pdu::encode("10086", &text, 9).unwrap();
            let mut decoded = pdu::decode(&parts[0].pdu).unwrap();
            decoded.direction = "received";
            import(&mut db, "m1", 1, &parts[0].pdu, &decoded).unwrap();
        }
        assert_eq!(
            list(&db, "m1", None, None, 50).unwrap()["items"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }
}
