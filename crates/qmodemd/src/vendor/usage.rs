// SPDX-License-Identifier: GPL-3.0-only
// Ported from FUjr/QModem vendor/quectel.sh usage functions.
use super::*;
pub fn command(device: &Modem, operation: &Operation) -> Result<Step> {
    ensure!(
        family(device)? == Family::Quectel,
        "modem traffic counters unavailable for MT5700"
    );
    let unisoc = device.platform == "unisoc";
    let command = match operation {
        Operation::GetUsageStats => {
            if unisoc {
                "AT+QGDCNT?"
            } else {
                "AT+QGDNRCNT?"
            }
        }
        Operation::WriteUsageStats => {
            if unisoc {
                "AT+QAUGDCNT=30"
            } else {
                "AT+QGDNRCNT=1"
            }
        }
        Operation::ClearUsageStats => {
            if unisoc {
                "AT+QGDCNT=0"
            } else {
                "AT+QGDNRCNT=0"
            }
        }
        _ => bail!("not a usage operation"),
    };
    Step::command(command, Duration::from_secs(10))
}
pub fn interpret(device: &Modem, reply: &Reply) -> Value {
    let unisoc = device.platform == "unisoc";
    let marker = if unisoc { "+QGDCNT:" } else { "+QGDNRCNT:" };
    let data = reply
        .response
        .lines()
        .find_map(|line| line.trim().strip_prefix(marker));
    let fields = data
        .unwrap_or("")
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>();
    let number = |index| {
        fields
            .get(index)
            .and_then(|s: &&str| s.parse::<u64>().ok())
            .unwrap_or(0)
    };
    json!({"available":data.is_some(),"total_rx_bytes":number(if unisoc{0}else{1}),"total_tx_bytes":number(if unisoc{1}else{0})})
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rx_and_tx_order_differs_by_platform() {
        let mut m:Modem=serde_json::from_value(json!({"id":"m","name":"test","manufacturer":"quectel","platform":"qualcomm","at_port":"/dev/test","bus":"usb"})).unwrap();
        let reply = |s: &str| Reply {
            status: 0,
            terminal: "OK".into(),
            modem_success: true,
            response: s.into(),
        };
        assert_eq!(
            interpret(&m, &reply("+QGDNRCNT: 12,34"))["total_rx_bytes"],
            34
        );
        m.platform = "unisoc".into();
        assert_eq!(
            interpret(&m, &reply("+QGDCNT: 12,34"))["total_rx_bytes"],
            12
        );
        assert_eq!(interpret(&m, &reply("ERROR"))["available"], false);
        assert_eq!(
            command(&m, &Operation::WriteUsageStats).unwrap().bytes,
            b"AT+QAUGDCNT=30\r\n"
        );
    }
}
