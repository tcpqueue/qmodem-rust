use super::*;
use crate::at::{Next, Program};
fn device(vendor: &str, platform: &str) -> Modem {
    serde_json::from_value(json!({"id":"m1","name":"test","manufacturer":vendor,"model":if vendor=="tdtech"{"mt5700m-cn"}else{"rm500u-cnv"},"platform":platform,"at_port":"/dev/ttyUSB2","bus":"usb"})).unwrap()
}
fn reply(raw: &str) -> Reply {
    let success = raw.lines().any(|l| l == "OK");
    Reply {
        status: 0,
        terminal: if success { "OK" } else { "ERROR" }.into(),
        modem_success: success,
        response: raw.into(),
    }
}
fn next_command(program: &mut dyn Program, replies: &[Reply]) -> String {
    match program.next(replies).unwrap() {
        Next::Command(s) => String::from_utf8(s.bytes).unwrap(),
        _ => panic!("expected command"),
    }
}
fn runtime() -> (tempfile::TempDir, Runtime) {
    let dir = tempfile::tempdir().unwrap();
    let state = Runtime::new(dir.path().join("runtime"));
    (dir, state)
}
#[test]
fn sim_parser_matches_upstream_digit_filter_and_alternate_spelling() {
    for (raw, expected) in [
        ("+QUIMSLOT: 1,2", None),
        ("+QUSIMSLOT: \"2\"", Some(2)),
        ("+QUIMSLOT: 12\n+QUIMSLOT: 1", Some(1)),
        ("+QUIMSLOT: 2:99", Some(2)),
        ("+QUIMSLOT: 0", None),
    ] {
        assert_eq!(sim::parse_slot(raw), expected, "{raw}");
    }
    assert_eq!(
        sim::capabilities("+QUSIMSLOT: (1,2)")["slots"],
        json!([1, 2])
    );
    // Upstream support flag uses nonempty raw text, not the filtered slot count.
    assert_eq!(sim::capabilities("+QUIMSLOT: (3)")["supported"], true);
    assert_eq!(sim::capabilities("ERROR")["supported"], false);
}
#[test]
fn sim_switch_stops_on_rejection_or_first_matching_read() {
    let (_dir, rt) = runtime();
    let d = device("quectel", "qualcomm");
    let op = Operation::SetSimSlot { slot: 2 };
    for accepted in [false, true] {
        let mut program = plan(&d, &op, &rt).unwrap();
        assert_eq!(next_command(&mut *program, &[]), "AT+QUIMSLOT=2\r\n");
        let mut replies = vec![reply(if accepted { "OK\r\n" } else { "ERROR\r\n" })];
        if accepted {
            assert_eq!(next_command(&mut *program, &replies), "AT+QUIMSLOT?\r\n");
            replies.push(reply("+QUSIMSLOT: 2\r\nOK\r\n"));
        }
        assert!(matches!(program.next(&replies).unwrap(), Next::Finish));
        assert_eq!(finish(&d, &op, &replies).unwrap()["success"], accepted);
    }
}
#[test]
fn sim_switch_has_exactly_five_failed_reads_and_five_delays() {
    let (_dir, rt) = runtime();
    let d = device("quectel", "qualcomm");
    let op = Operation::SetSimSlot { slot: 2 };
    let mut program = plan(&d, &op, &rt).unwrap();
    next_command(&mut *program, &[]);
    let mut replies = vec![reply("OK\r\n")];
    for attempt in 0..5 {
        assert_eq!(next_command(&mut *program, &replies), "AT+QUIMSLOT?\r\n");
        replies.push(reply(if attempt == 2 {
            "ERROR\r\n"
        } else {
            "+QUIMSLOT: 1\r\nOK\r\n"
        }));
        assert!(
            matches!(program.next(&replies).unwrap(),Next::Wait(d) if d==Duration::from_secs(1))
        );
    }
    assert!(matches!(program.next(&replies).unwrap(), Next::Finish));
    let result = finish(&d, &op, &replies).unwrap();
    assert_eq!(result["success"], false);
    assert_eq!(result["data"]["attempts"], 5);
    assert_eq!(result["data"]["sim_slot"], 1);
    assert_eq!(result["error_code"], "sim_switch_unconfirmed");
}
#[test]
fn mt5700_software_state_is_written_before_at_and_survives_rejection_and_restart() {
    let (dir, rt) = runtime();
    let d = device("tdtech", "hisilicon");
    assert_eq!(
        local(&d, &Operation::GetSimSlot, &rt).unwrap().unwrap()["data"]["sim_slot"],
        0
    );
    for slot in [1, 0] {
        let op = Operation::SetSimSlot { slot };
        let mut program = plan(&d, &op, &rt).unwrap();
        let cmd = next_command(&mut *program, &[]);
        assert_eq!(
            cmd,
            if slot == 1 {
                "AT^SCICHG=1,0\r\n"
            } else {
                "AT^SCICHG=0,1\r\n"
            }
        );
        let restart = Runtime::new(dir.path().join("runtime"));
        assert_eq!(restart.slot("m1").unwrap(), slot);
        let result = finish(&d, &op, &[reply("ERROR\r\n")]).unwrap();
        assert_eq!(result["success"], false);
        assert_eq!(result["data"]["hardware_verified"], false);
        assert_eq!(restart.slot("m1").unwrap(), slot);
    }
    assert_eq!(rt.slot("another-modem").unwrap(), 0);
    let (_reboot_dir, reboot) = runtime();
    assert_eq!(reboot.slot("m1").unwrap(), 0);
}
#[test]
fn software_state_failure_prevents_at_dispatch() {
    use std::os::unix::fs::symlink;
    let (dir, _rt) = runtime();
    let target = dir.path().join("target");
    std::fs::create_dir(&target).unwrap();
    let link = dir.path().join("link");
    symlink(&target, &link).unwrap();
    let rt = Runtime::new(link);
    let mut program = plan(
        &device("tdtech", "hisilicon"),
        &Operation::SetSimSlot { slot: 1 },
        &rt,
    )
    .unwrap();
    assert!(program.next(&[]).is_err());
    assert!(std::fs::read_dir(target).unwrap().next().is_none());
}
#[test]
fn band_platform_dispatch_and_large_mask_do_not_lose_high_bits() {
    let (_dir, rt) = runtime();
    for (platform, count) in [
        ("lte", 1),
        ("hisilicon", 1),
        ("qualcomm", 4),
        ("unisoc", 4),
        ("lte12", 4),
    ] {
        let steps = bands::query(&device("quectel", platform)).unwrap();
        assert_eq!(steps.len(), count);
    }
    let lte = device("quectel", "lte");
    let step = bands::setter(&lte, bands::Class::Lte, &[71, 1, 66, 1]).unwrap();
    assert_eq!(
        String::from_utf8(step.bytes).unwrap(),
        "AT+QCFG=\"band\",0,420000000000000001,0\r\n"
    );
    let parsed = bands::interpret(
        &lte,
        &[reply("+QCFG: \"band\",0,420000000000000001,0\r\nOK\r\n")],
    )
    .unwrap();
    assert_eq!(
        parsed["data"]["bands"][0]["locked_bands"],
        json!([1, 66, 71])
    );
    for platform in ["qualcomm", "unisoc", "lte12", "hisilicon"] {
        let step = bands::setter(
            &device("quectel", platform),
            bands::Class::Nsa,
            &[78, 41, 78],
        )
        .unwrap();
        assert_eq!(
            String::from_utf8(step.bytes).unwrap(),
            "AT+QNWPREFCFG=\"nsa_nr5g_band\",78:41:78\r\n"
        );
    }
    assert!(plan(&device("tdtech", "hisilicon"), &Operation::GetBandLock, &rt).is_err());
    assert!(bands::setter(&lte, bands::Class::Lte, &[0]).is_err());
    assert!(bands::setter(&lte, bands::Class::Lte, &[1025]).is_err());
}
fn fixture(name: &str) -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/quectel-rm500u")
        .join(name);
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}
fn decode_hex(hex: &str) -> String {
    String::from_utf8(
        hex.as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u8::from_str_radix(std::str::from_utf8(c).unwrap(), 16).unwrap())
            .collect(),
    )
    .unwrap()
}
#[test]
fn upstream_rm500u_recordings_match_band_order_and_expected_output() {
    let (_dir, rt) = runtime();
    let d = device("quectel", "unisoc");
    let mut program = plan(&d, &Operation::GetBandLock, &rt).unwrap();
    let mut replies = Vec::new();
    for name in [
        "AT_QNWPREFCFG__gw_band_-242f6fd6.json",
        "AT_QNWPREFCFG__lte_band_-2a907e37.json",
        "AT_QNWPREFCFG__nsa_nr5g_band_-b8b37991.json",
        "AT_QNWPREFCFG__nr5g_band_-abfb1f83.json",
    ] {
        let f = fixture(name);
        assert_eq!(
            next_command(&mut *program, &replies),
            format!("{}\r\n", f["command"].as_str().unwrap())
        );
        replies.push(reply(&decode_hex(f["response_hex"].as_str().unwrap())));
    }
    assert!(!replies[2].modem_success);
    assert!(matches!(program.next(&replies).unwrap(), Next::Finish));
    let result = finish(&d, &Operation::GetBandLock, &replies).unwrap();
    let expected = fixture("expected-lockband.json");
    assert_eq!(result["success"], true);
    assert_eq!(result["data"]["partial"], false);
    assert_eq!(result["data"]["bands"].as_array().unwrap().len(), 3);
    for entry in result["data"]["bands"].as_array().unwrap() {
        let class = entry["band_class"].as_str().unwrap();
        let old: Vec<u16> = expected["lockband"][class]["lock_band"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().parse().unwrap())
            .collect();
        assert_eq!(entry["locked_bands"], json!(old));
        let available: Vec<u16> = expected["lockband"][class]["available_band"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["band_id"].as_str().unwrap().parse().unwrap())
            .collect();
        assert_eq!(entry["available_bands"], json!(available));
    }
    let slot = fixture("AT_QUIMSLOT_-8ba74f4e.json");
    assert_eq!(
        sim::parse_slot(&decode_hex(slot["response_hex"].as_str().unwrap())),
        Some(1)
    );
    let caps = fixture("AT_QUIMSLOT__-f84a17ce.json");
    assert_eq!(
        sim::capabilities(&decode_hex(caps["response_hex"].as_str().unwrap()))["slots"],
        json!([1, 2])
    );
}
#[test]
fn partial_band_failure_is_unknown_not_an_empty_unlocked_list() {
    let mut d = device("quectel", "qualcomm");
    d.bands.lte = Some(vec![3, 71]);
    let result = bands::interpret(
        &d,
        &[
            reply("ERROR\r\n"),
            reply("+QNWPREFCFG: \"lte_band\",3\r\nOK\r\n"),
            reply("OK\r\n"),
            reply("ERROR\r\n"),
        ],
    )
    .unwrap();
    assert_eq!(result["success"], true);
    assert_eq!(result["data"]["partial"], true);
    assert_eq!(result["data"]["bands"][0]["locked_bands"], Value::Null);
    assert_eq!(
        result["data"]["bands"][1]["available_bands"],
        json!([3, 71])
    );
    assert_eq!(
        result["data"]["bands"][1]["available_source"],
        "configuration"
    );
    assert_eq!(result["data"]["bands"][2]["state"], "unknown");
}
#[test]
fn quectel_imei_readback_runs_even_after_setter_error_and_tdtech_keeps_its_command() {
    let (_dir, rt) = runtime();
    let op = Operation::SetImei {
        imei: "123456789012345".into(),
    };
    let mut p = plan(&device("quectel", "qualcomm"), &op, &rt).unwrap();
    assert_eq!(
        next_command(&mut *p, &[]),
        "AT+EGMR=1,7,\"123456789012345\"\r\n"
    );
    assert_eq!(next_command(&mut *p, &[reply("ERROR\r\n")]), "AT+CGSN\r\n");
    let result = finish(
        &device("quectel", "qualcomm"),
        &op,
        &[reply("ERROR\r\n"), reply("123456789012345\r\nOK\r\n")],
    )
    .unwrap();
    assert_eq!(result["success"], false);
    assert_eq!(result["data"]["matches_requested"], true);
    let mut p = plan(&device("tdtech", "hisilicon"), &op, &rt).unwrap();
    assert_eq!(
        next_command(&mut *p, &[]),
        "at^phynum=IMEI,123456789012345\r\n"
    );
}

#[test]
fn imei_verification_cannot_report_a_mismatched_or_missing_readback_as_success() {
    let d = device("quectel", "qualcomm");
    let op = Operation::SetImei {
        imei: "123456789012345".into(),
    };
    for (raw, code) in [
        ("987654321098765\r\nOK\r\n", "imei_unconfirmed"),
        ("OK\r\n", "invalid_modem_response"),
    ] {
        let result = finish(&d, &op, &[reply("OK\r\n"), reply(raw)]).unwrap();
        assert_eq!(result["success"], false);
        assert_eq!(result["error_code"], code);
    }
}
