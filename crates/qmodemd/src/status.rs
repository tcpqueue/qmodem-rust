// SPDX-License-Identifier: GPL-3.0-only
// Status queries and field mappings derived from FUjr/QModem vendor scripts.
use crate::{
    at::{AtError, Next, PortPool, Program, Reply, Step},
    config::Modem,
    vendor::{self, Family, cells::fields},
};
use anyhow::Result;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    sync::{Arc, Mutex as StdMutex},
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

type Records = BTreeMap<String, Reply>;
type Entry = Arc<Mutex<Option<(Instant, u64, Value)>>>;
#[derive(Default)]
pub struct Cache {
    entries: Mutex<HashMap<String, Entry>>,
    generation: std::sync::atomic::AtomicU64,
}
impl Cache {
    pub async fn get(&self, modem: &Modem, ports: &PortPool) -> Result<Value> {
        let key = serde_json::to_string(modem)?;
        let entry = {
            let mut entries = self.entries.lock().await;
            if entries.len() > 128 {
                entries.retain(|_, v| Arc::strong_count(v) > 1);
            }
            entries.entry(key).or_default().clone()
        };
        let mut cached = entry.lock().await;
        let generation = self.generation.load(std::sync::atomic::Ordering::Acquire);
        if let Some((time, _, value)) = cached
            .as_ref()
            .filter(|(t, g, _)| *g == generation && t.elapsed() < Duration::from_secs(3))
        {
            let mut value = value.clone();
            value["cache_age_ms"] = json!(time.elapsed().as_millis() as u64);
            return Ok(value);
        }
        let records = Arc::new(StdMutex::new(Records::new()));
        let program = StatusProgram::new(modem, records.clone())?;
        let port = ports.get(&modem.at_port).await?;
        port.run_named(Box::new(program), Some(modem.id.clone()), "status")
            .await?;
        let value = report(modem, &records.lock().unwrap());
        *cached = Some((Instant::now(), generation, value.clone()));
        Ok(value)
    }
    pub async fn invalidate(&self) {
        self.generation
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    }
}
struct StatusProgram {
    quectel: bool,
    todo: VecDeque<(String, String)>,
    current: Option<String>,
    recorded: usize,
    records: Arc<StdMutex<Records>>,
}
impl StatusProgram {
    fn new(modem: &Modem, records: Arc<StdMutex<Records>>) -> Result<Self> {
        let quectel = vendor::family(modem)? == Family::Quectel;
        let mut p = Self {
            quectel,
            todo: VecDeque::new(),
            current: None,
            recorded: 0,
            records,
        };
        for (key, cmd) in [
            ("model", "AT+CGMM"),
            ("manufacturer", "AT+CGMI"),
            ("revision", "ATI"),
        ] {
            p.add(key, cmd);
        }
        if quectel {
            p.add("temperature", "AT+QTEMP");
            p.add("voltage", "AT+CBC");
        }
        p.add("activation", "AT+CGACT?");
        if !quectel {
            p.add("temperature", "AT^CHIPTEMP?");
        }
        if quectel {
            p.add("sim_slot", "AT+QUIMSLOT?");
            p.add("imei", "AT+CGSN");
        }
        p.add("sim_status", "AT+CPIN?");
        if !quectel {
            p.add("number", "AT+CNUM");
            p.add("imsi", "AT+CIMI");
            p.add("imei", "AT+CGSN");
        }
        Ok(p)
    }
    fn add(&mut self, key: &str, cmd: &str) {
        self.todo.push_back((key.into(), cmd.into()));
    }
    fn follow(&mut self, key: &str, reply: &Reply) {
        match key {
            "activation" => {
                for line in reply
                    .response
                    .lines()
                    .filter_map(|l| l.trim().strip_prefix("+CGACT:"))
                {
                    let f = fields(line);
                    if f.get(1).is_some_and(|s| s == "1")
                        && let Some(cid) = f
                            .first()
                            .and_then(|s| s.parse::<u8>().ok())
                            .filter(|n| *n <= 16)
                    {
                        self.todo
                            .push_front((format!("address_{cid}"), format!("AT+CGPADDR={cid}")));
                    }
                }
            }
            "sim_status" if reply.response.contains("+CPIN: READY") => {
                if self.quectel {
                    for (key, cmd) in [
                        ("operator_format", "AT+COPS=3,2"),
                        ("operator", "AT+COPS?"),
                        ("number", "AT+CNUM"),
                        ("imsi", "AT+CIMI"),
                        ("iccid", "AT+ICCID"),
                        ("network_type", "AT+QNWINFO"),
                        ("signal", "AT+CSQ"),
                        ("ambr", "AT+QNWCFG=\"nr5g_ambr\""),
                        ("speed", "AT+QNWCFG=\"up/down\""),
                        ("serving", "AT+QENG=\"servingcell\""),
                    ] {
                        self.add(key, cmd);
                    }
                } else {
                    self.add("serving", "AT^MONSC");
                }
            }
            "iccid" if !reply.response.contains("+ICCID:") => {
                self.todo
                    .push_front(("iccid_fallback".into(), "AT+CCID".into()));
            }
            "network_type" if !reply.response.contains("+QNWINFO:") => {
                self.todo
                    .push_front(("network_fallback".into(), "AT+COPS?".into()));
            }
            "serving" if self.quectel && reply.response.contains("NR5G-SA") => {
                self.add("aggregation", "AT+QCAINFO");
            }
            "serving" if !self.quectel && reply.response.contains("LTE-NR") => {
                self.add("secondary_signal", "AT^CSERSSI?");
            }
            _ => {}
        }
    }
}
impl Program for StatusProgram {
    fn next(&mut self, replies: &[Reply]) -> std::result::Result<Next, AtError> {
        if replies.len() > self.recorded {
            let reply = replies.last().unwrap();
            let key = self.current.take().unwrap();
            self.records
                .lock()
                .unwrap()
                .insert(key.clone(), reply.clone());
            self.recorded = replies.len();
            self.follow(&key, reply);
        }
        let Some((key, command)) = self.todo.pop_front() else {
            return Ok(Next::Finish);
        };
        self.current = Some(key);
        Ok(Next::Command(
            Step::command(&command, Duration::from_secs(5)).expect("validated status command"),
        ))
    }
}
fn pref<'a>(records: &'a Records, key: &str, prefix: &str) -> Option<&'a str> {
    records
        .get(key)
        .filter(|r| r.modem_success)?
        .response
        .lines()
        .find_map(|l| l.trim().strip_prefix(prefix))
        .map(str::trim)
}
fn scalar(records: &Records, key: &str) -> Option<String> {
    records
        .get(key)
        .filter(|r| r.modem_success)?
        .response
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with("AT") && *l != "OK")
        .map(str::to_owned)
}
fn report(modem: &Modem, records: &Records) -> Value {
    let quectel = modem.manufacturer.eq_ignore_ascii_case("quectel");
    let temp = if quectel {
        records
            .get("temperature")
            .and_then(|r| {
                r.response
                    .lines()
                    .filter_map(|l| l.trim().strip_prefix("+QTEMP:"))
                    .flat_map(|l| l.split(|c: char| !c.is_ascii_digit()))
                    .filter_map(|v| v.parse::<u16>().ok())
                    .rfind(|n| *n > 10 && *n < 110)
            })
            .map(f64::from)
    } else {
        pref(records, "temperature", "^CHIPTEMP:")
            .and_then(|l| fields(l).get(5).and_then(|s| s.parse::<f64>().ok()))
            .map(|value| value / 10.0)
    };
    let voltage = pref(records, "voltage", "+CBC:")
        .and_then(|l| fields(l).get(2).and_then(|v| v.parse::<u32>().ok()));
    let sim_status = pref(records, "sim_status", "+CPIN:")
        .map(str::to_ascii_lowercase)
        .or_else(|| {
            records
                .get("sim_status")
                .filter(|r| r.response.contains("+CME ERROR: 10"))
                .map(|_| "not inserted".into())
        });
    let csq = pref(records, "signal", "+CSQ:")
        .and_then(|l| fields(l).first().and_then(|s| s.parse::<u8>().ok()))
        .filter(|n| *n <= 31);
    let operator = pref(records, "operator", "+COPS:").and_then(|l| fields(l).get(2).cloned());
    let iccid =
        pref(records, "iccid", "+ICCID:").or_else(|| pref(records, "iccid_fallback", "+CCID:"));
    let addresses = records
        .iter()
        .filter(|(key, _)| key.starts_with("address_"))
        .flat_map(|(_, reply)| {
            reply
                .response
                .lines()
                .filter_map(|l| l.trim().strip_prefix("+CGPADDR:"))
                .flat_map(|l| fields(l).into_iter().skip(1))
        })
        .filter(|s| {
            s.parse::<std::net::IpAddr>()
                .is_ok_and(|ip| !ip.is_unspecified())
        })
        .collect::<Vec<_>>();
    let cells = if quectel {
        quectel_cells(records)
    } else {
        mt_cells(records)
    };
    let raw = records
        .iter()
        .map(|(key, reply)| {
            (
                key.clone(),
                json!({"success":reply.modem_success,"response":reply.response}),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    json!({"modem_id":modem.id,"sampled_at":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs(),"cache_age_ms":0,
        "model":scalar(records,"model"),"manufacturer":scalar(records,"manufacturer"),"firmware":pref(records,"revision","Revision:"),
        "temperature_c":temp,"voltage_mv":voltage,"sim_status":sim_status,"imei":scalar(records,"imei"),"imsi":scalar(records,"imsi"),"iccid":iccid,
        "phone_number":pref(records,"number","+CNUM:").and_then(|l|fields(l).get(1).cloned()),"operator":operator,
        "sim_slot":pref(records,"sim_slot","+QUIMSLOT:").or_else(||pref(records,"sim_slot","+QUSIMSLOT:")),
        "network_type":pref(records,"network_type","+QNWINFO:").and_then(|l|fields(l).first().cloned()).or_else(||if quectel{None}else{cells.first().and_then(|c|c["rat"].as_str().map(str::to_owned))}),"csq":csq,"rssi_dbm":csq.map(|n|-113+2*i16::from(n)),
        "pdp_active":!addresses.is_empty(),"addresses":addresses,"cells":cells,"partial":records.values().any(|r|!r.modem_success),"queries":raw})
}
fn map_fields(f: &[String], pairs: &[(&str, usize)]) -> Value {
    let mut value = serde_json::Map::new();
    for (key, index) in pairs {
        value.insert((*key).into(), json!(f.get(*index)));
    }
    value.insert("fields".into(), json!(f));
    Value::Object(value)
}
fn quectel_cells(records: &Records) -> Vec<Value> {
    let mut result = Vec::new();
    if let Some(reply) = records.get("serving") {
        for line in reply
            .response
            .lines()
            .filter_map(|l| l.trim().strip_prefix("+QENG:"))
        {
            let f = fields(line);
            let first = f.first().map(String::as_str).unwrap_or("");
            let pairs: &[(&str, usize)] = match first {
                "LTE" => &[
                    ("rat", 0),
                    ("duplex", 1),
                    ("mcc", 2),
                    ("mnc", 3),
                    ("cell_id", 4),
                    ("pci", 5),
                    ("arfcn", 6),
                    ("band", 7),
                    ("ul_bandwidth_index", 8),
                    ("dl_bandwidth_index", 9),
                    ("tac", 10),
                    ("rsrp", 11),
                    ("rsrq", 12),
                    ("rssi", 13),
                    ("sinr", 14),
                    ("cqi", 15),
                    ("tx_power", 16),
                    ("srxlev", 17),
                ],
                "NR5G-NSA" => &[
                    ("rat", 0),
                    ("mcc", 1),
                    ("mnc", 2),
                    ("pci", 3),
                    ("rsrp", 4),
                    ("sinr", 5),
                    ("rsrq", 6),
                    ("arfcn", 7),
                    ("band", 8),
                    ("dl_bandwidth_index", 9),
                    ("scs_index", 15),
                ],
                "servingcell" => match f.get(2).map(String::as_str) {
                    Some("NR5G-SA") => &[
                        ("rat", 2),
                        ("duplex", 3),
                        ("mcc", 4),
                        ("mnc", 5),
                        ("cell_id", 6),
                        ("pci", 7),
                        ("tac", 8),
                        ("arfcn", 9),
                        ("band", 10),
                        ("rsrp", 12),
                        ("rsrq", 13),
                        ("sinr", 14),
                        ("scs_index", 15),
                        ("srxlev", 16),
                    ],
                    Some("LTE" | "CAT-M" | "CAT-NB") => &[
                        ("rat", 2),
                        ("duplex", 3),
                        ("mcc", 4),
                        ("mnc", 5),
                        ("cell_id", 6),
                        ("pci", 7),
                        ("arfcn", 8),
                        ("band", 9),
                        ("ul_bandwidth_index", 10),
                        ("dl_bandwidth_index", 11),
                        ("tac", 12),
                        ("rsrp", 13),
                        ("rsrq", 14),
                        ("rssi", 15),
                        ("sinr", 16),
                        ("cqi", 17),
                        ("tx_power", 18),
                        ("srxlev", 19),
                    ],
                    Some("WCDMA") => &[
                        ("rat", 2),
                        ("mcc", 3),
                        ("mnc", 4),
                        ("lac", 5),
                        ("cell_id", 6),
                        ("arfcn", 7),
                        ("psc", 8),
                        ("rscp", 9),
                        ("ecio", 10),
                    ],
                    _ => continue,
                },
                _ => continue,
            };
            result.push(map_fields(&f, pairs));
        }
    }
    if let Some(reply) = records.get("aggregation") {
        for line in reply
            .response
            .lines()
            .filter_map(|l| l.trim().strip_prefix("+QCAINFO:"))
        {
            let f = fields(line);
            if f.first().is_some_and(|s| s == "SCC") {
                let mut c = map_fields(
                    &f,
                    &[
                        ("role", 0),
                        ("arfcn", 1),
                        ("dl_bandwidth_index", 2),
                        ("band", 3),
                        ("pci", 5),
                    ],
                );
                c["rat"] = json!("NR");
                result.push(c);
            }
        }
    }
    result
}
fn mt_cells(records: &Records) -> Vec<Value> {
    let Some(line) = pref(records, "serving", "^MONSC:") else {
        return vec![];
    };
    let f = fields(line);
    let pairs: &[(&str, usize)] = match f.first().map(String::as_str) {
        Some("NR" | "NR-5GC") => &[
            ("rat", 0),
            ("mcc", 1),
            ("mnc", 2),
            ("arfcn", 3),
            ("scs_index", 4),
            ("cell_id_hex", 5),
            ("pci_hex", 6),
            ("tac", 7),
            ("rsrp", 8),
            ("rsrq", 9),
            ("sinr", 10),
        ],
        Some("LTE" | "LTE-NR" | "eMTC" | "NB-IoT") => &[
            ("rat", 0),
            ("mcc", 1),
            ("mnc", 2),
            ("arfcn", 3),
            ("cell_id_hex", 4),
            ("pci_hex", 5),
            ("tac", 6),
            ("rsrp", 7),
            ("rsrq", 8),
            ("rxlev", 9),
        ],
        Some("WCDMA" | "TD-SCDMA" | "UMTS") => &[
            ("rat", 0),
            ("mcc", 1),
            ("mnc", 2),
            ("arfcn", 3),
            ("psc", 4),
            ("cell_id_hex", 5),
            ("lac", 6),
            ("rscp", 7),
            ("rxlev", 8),
            ("ecno", 9),
            ("drx", 10),
            ("ura", 11),
        ],
        _ => &[("rat", 0)],
    };
    let mut cell = map_fields(&f, pairs);
    for field in ["cell_id", "pci"] {
        let number = cell[format!("{field}_hex")]
            .as_str()
            .and_then(|s| u64::from_str_radix(s, 16).ok());
        cell[field] = json!(number);
    }
    let mut result = vec![cell];
    if let Some(reply) = records.get("secondary_signal") {
        let f = fields(&reply.response);
        let mut cell = map_fields(&f, &[("rsrp", 11), ("rsrq", 12), ("sinr", 13)]);
        cell["rat"] = json!("NR5G-NSA");
        result.push(cell);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn modem() -> Modem {
        serde_json::from_value(json!({"id":"m","name":"test","manufacturer":"quectel","platform":"qualcomm","at_port":"/dev/test","bus":"usb"})).unwrap()
    }
    fn reply(s: &str) -> Reply {
        Reply {
            status: 0,
            terminal: "OK".into(),
            modem_success: !s.contains("ERROR"),
            response: s.into(),
        }
    }
    #[test]
    fn non_ready_sim_skips_operator_and_radio_queries() {
        let records = Arc::new(StdMutex::new(Records::new()));
        let mut program = StatusProgram::new(&modem(), records).unwrap();
        let mut replies = vec![];
        let mut commands = vec![];
        loop {
            match program.next(&replies).unwrap() {
                Next::Finish => break,
                Next::Command(step) => {
                    commands.push(String::from_utf8(step.bytes).unwrap());
                    replies.push(reply("+CPIN: SIM PIN\r\nOK"));
                }
                _ => panic!("unexpected wait"),
            }
        }
        assert!(commands.iter().any(|c| c == "AT+CPIN?\r\n"));
        assert!(
            !commands
                .iter()
                .any(|c| c.contains("COPS") || c.contains("QENG"))
        );
    }
    #[test]
    fn iccid_fallback_and_sa_aggregation_are_conditional() {
        let records = Arc::new(StdMutex::new(Records::new()));
        let mut program = StatusProgram::new(&modem(), records.clone()).unwrap();
        let mut replies = vec![];
        let mut commands = vec![];
        loop {
            match program.next(&replies).unwrap() {
                Next::Finish => break,
                Next::Command(step) => {
                    let cmd = String::from_utf8(step.bytes).unwrap();
                    let response = match cmd.trim() {
                        "AT+CPIN?" => "+CPIN: READY\r\nOK",
                        "AT+ICCID" => "ERROR",
                        "AT+CCID" => "+CCID: 89860123456789012345\r\nOK",
                        "AT+QENG=\"servingcell\"" => {
                            "+QENG: \"servingcell\",\"NOCONN\",\"NR5G-SA\",\"TDD\",460,01,A1234,113,01,627264,78,12,-92,-11,21,1,30\r\nOK"
                        }
                        _ => "OK",
                    };
                    commands.push(cmd);
                    replies.push(reply(response));
                }
                _ => panic!(),
            }
        }
        let idx = commands.iter().position(|c| c == "AT+ICCID\r\n").unwrap();
        assert_eq!(commands[idx + 1], "AT+CCID\r\n");
        assert_eq!(commands.last().unwrap(), "AT+QCAINFO\r\n");
        let status = report(&modem(), &records.lock().unwrap());
        assert_eq!(status["iccid"], "89860123456789012345");
        assert_eq!(status["cells"][0]["arfcn"], "627264");
        assert_eq!(status["cells"][0]["rsrp"], "-92");
        assert!(status["voltage_mv"].is_null());
    }
    #[test]
    fn nsa_components_and_mt5700_hex_fields_are_separate() {
        let mut records = Records::new();
        records.insert("serving".into(),reply("+QENG: \"LTE\",\"FDD\",460,01,AB,50,1850,3,5,5,123,-94,-12,-65,15\r\n+QENG: \"NR5G-NSA\",460,01,113,-92,21,-11,627264,78,12"));
        let cells = quectel_cells(&records);
        assert_eq!(cells.len(), 2);
        assert_eq!(cells[0]["band"], "3");
        assert_eq!(cells[1]["rsrq"], "-11");
        records.insert(
            "serving".into(),
            reply("^MONSC: NR,460,01,627264,1,A1234,71,01,-92,-11,21"),
        );
        let cells = mt_cells(&records);
        assert_eq!(cells[0]["pci"], 113);
        assert_eq!(cells[0]["cell_id"], 660020);
    }
    #[test]
    fn mt5700_b024_hardware_temperature_and_nr_response() {
        let mut m = modem();
        m.manufacturer = "tdtech".into();
        m.model = "mt5700m-cn".into();
        m.platform = "hisilicon".into();
        let mut records = Records::new();
        records.insert(
            "model".into(),
            reply(
                "MT5700M-CN
OK",
            ),
        );
        records.insert(
            "revision".into(),
            reply(
                "Manufacturer: TD Tech Ltd.
Model: MT5700M-CN
Revision: V200R001C20B024
OK",
            ),
        );
        records.insert(
            "sim_status".into(),
            reply(
                "+CPIN: READY
OK",
            ),
        );
        records.insert(
            "temperature".into(),
            reply(
                "^CHIPTEMP: 336,331,330,339,320,320,330,340,330,340,320,320
OK",
            ),
        );
        // Cell and location identifiers are replaced; field width and radio metrics are retained.
        records.insert(
            "serving".into(),
            reply(
                "^MONSC: NR,460,11,633984,1,A1234500C,F9,ABCDEF,-94,-10,30
OK",
            ),
        );
        let value = report(&m, &records);
        assert_eq!(value["temperature_c"], 32.0);
        assert_eq!(value["firmware"], "V200R001C20B024");
        assert_eq!(value["sim_status"], "ready");
        assert_eq!(value["network_type"], "NR");
        assert_eq!(value["cells"][0]["pci"], 249);
        assert_eq!(value["cells"][0]["rsrp"], "-94");
        assert_eq!(value["cells"][0]["rsrq"], "-10");
        assert_eq!(value["cells"][0]["sinr"], "30");
    }
}
