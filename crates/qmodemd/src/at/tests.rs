use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt, duplex};
use tokio::time::sleep;
fn command(s: &str) -> Step {
    Step::command(s, Duration::from_secs(1)).unwrap()
}

#[tokio::test]
async fn fragmented_reply_and_modem_error_are_preserved() {
    let (client, mut modem) = duplex(4096);
    let port = Port::start(client);
    tokio::spawn(async move {
        let mut buf = [0; 4];
        modem.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"AT\r\n");
        for piece in [b"\r\nBROKEN\r\n+C".as_slice(), b"ME ERROR: 10\r\n"] {
            modem.write_all(piece).await.unwrap();
            sleep(Duration::from_millis(5)).await;
        }
    });
    let r = port.execute(vec![command("AT")]).await.unwrap();
    assert_eq!(r[0].terminal, "+CME ERROR:");
    assert!(!r[0].modem_success);
    assert_eq!(r[0].status, 0);
    assert!(r[0].response.contains("BROKEN"));
}
#[tokio::test]
async fn sms_prompt_and_payload_are_one_transaction() {
    let (client, mut modem) = duplex(4096);
    let port = Port::start(client);
    let simulator = tokio::spawn(async move {
        let mut head = [0; 11];
        modem.read_exact(&mut head).await.unwrap();
        assert_eq!(&head, b"AT+CMGS=4\r\n");
        modem.write_all(b"\r\n> ").await.unwrap();
        let mut pdu = [0; 9];
        modem.read_exact(&mut pdu).await.unwrap();
        assert_eq!(&pdu, b"00010203\x1a");
        modem.write_all(b"\r\n+CMGS: 7\r\nOK\r\n").await.unwrap();
    });
    let mut prompt = command("AT+CMGS=4");
    prompt.flags = vec![">".into(), "ERROR".into(), "+CMS ERROR:".into()];
    let payload = Step {
        bytes: b"00010203\x1a".to_vec(),
        flags: default_flags(),
        timeout: Duration::from_secs(1),
    };
    let replies = port.execute(vec![prompt, payload]).await.unwrap();
    assert_eq!(replies.len(), 2);
    assert_eq!(replies[1].terminal, "OK");
    simulator.await.unwrap();
}
#[tokio::test]
async fn same_port_serializes_even_when_requester_is_cancelled() {
    let (client, mut modem) = duplex(4096);
    let port = Port::start(client);
    let cloned = port.clone();
    let started = Arc::new(tokio::sync::Notify::new());
    let notify = started.clone();
    let simulator = tokio::spawn(async move {
        let mut buf = [0; 5];
        modem.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"AT1\r\n");
        notify.notify_one();
        sleep(Duration::from_millis(30)).await;
        modem.write_all(b"first\r\nOK\r\n").await.unwrap();
        modem.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"AT2\r\n");
        modem.write_all(b"second\r\nOK\r\n").await.unwrap();
    });
    let first = tokio::spawn(async move { cloned.execute(vec![command("AT1")]).await });
    started.notified().await;
    first.abort();
    let reply = port.execute(vec![command("AT2")]).await.unwrap();
    assert!(reply[0].response.contains("second"));
    assert!(!reply[0].response.contains("first"));
    simulator.await.unwrap();
}
#[tokio::test]
async fn late_terminal_is_drained_before_next_command() {
    let (client, mut modem) = duplex(4096);
    let port = Port::start(client);
    let simulator = tokio::spawn(async move {
        let mut buf = [0; 5];
        modem.read_exact(&mut buf).await.unwrap();
        sleep(Duration::from_millis(80)).await;
        modem.write_all(b"late-first\r\nOK\r\n").await.unwrap();
        modem.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"AT2\r\n");
        modem.write_all(b"second\r\nOK\r\n").await.unwrap();
    });
    let mut first = command("AT1");
    first.timeout = Duration::from_millis(20);
    assert_eq!(
        port.execute(vec![first]).await.unwrap_err().kind,
        ErrorKind::Timeout
    );
    let reply = port.execute(vec![command("AT2")]).await.unwrap();
    assert!(!reply[0].response.contains("late-first"));
    assert!(reply[0].response.contains("second"));
    simulator.await.unwrap();
}
#[tokio::test]
async fn different_ports_do_not_block_each_other() {
    let (c1, _m1) = duplex(4096);
    let (c2, mut m2) = duplex(4096);
    let p1 = Port::start(c1);
    let p2 = Port::start(c2);
    let blocked = tokio::spawn(async move { p1.execute(vec![command("AT1")]).await });
    tokio::spawn(async move {
        let mut b = [0; 5];
        m2.read_exact(&mut b).await.unwrap();
        m2.write_all(b"OK\r\n").await.unwrap();
    });
    let result = tokio::time::timeout(Duration::from_millis(100), p2.execute(vec![command("AT2")]))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result[0].terminal, "OK");
    blocked.abort();
}
#[test]
fn command_injection_and_invalid_deadlines_are_rejected() {
    for c in ["AT\rAT+CFUN=1,1", "AT\n", "AT\0", "echo hi"] {
        assert!(Step::command(c, Duration::from_secs(1)).is_err());
    }
    assert!(Step::command("AT", Duration::ZERO).is_err());
}

#[tokio::test]
async fn native_kernel_pty_transport_handles_fragmented_responses() {
    let (mut master, slave) = tokio_serial::SerialStream::pair().unwrap();
    let port = Port::start(slave);
    let simulator = tokio::spawn(async move {
        let mut cmd = [0; 4];
        master.read_exact(&mut cmd).await.unwrap();
        assert_eq!(&cmd, b"AT\r\n");
        master.write_all(b"\r\nO").await.unwrap();
        sleep(Duration::from_millis(5)).await;
        master.write_all(b"K\r\n").await.unwrap();
        sleep(Duration::from_millis(30)).await;
    });
    let reply = port.execute(vec![command("AT")]).await.unwrap();
    assert_eq!(reply[0].terminal, "OK");
    simulator.await.unwrap();
}
#[tokio::test]
async fn unsolicited_events_preserve_fragmented_lines_without_using_response_buffer() {
    let (client, mut modem) = duplex(4096);
    let port = Port::start(client);
    let mut events = port.subscribe();
    modem.write_all(b"\r\n+CMT").await.unwrap();
    sleep(Duration::from_millis(5)).await;
    modem.write_all(b"I: \"SM\",7\r\n").await.unwrap();
    let event = tokio::time::timeout(Duration::from_secs(1), events.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(event.correlation, "unsolicited");
    assert_eq!(event.line, "+CMTI: \"SM\",7");
}

#[tokio::test]
async fn trailing_urc_in_same_read_is_not_discarded() {
    let (client, mut modem) = duplex(4096);
    let port = Port::start(client);
    let mut events = port.subscribe();
    let simulator = tokio::spawn(async move {
        let mut cmd = [0; 4];
        modem.read_exact(&mut cmd).await.unwrap();
        modem.write_all(b"OK\r\n+CMTI: \"SM\",3\r\n").await.unwrap();
        sleep(Duration::from_millis(20)).await;
    });
    assert_eq!(
        port.execute(vec![command("AT")]).await.unwrap()[0].terminal,
        "OK"
    );
    let terminal = events.recv().await.unwrap();
    assert_eq!(terminal.correlation, "terminal");
    let urc = tokio::time::timeout(Duration::from_millis(100), events.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(urc.correlation, "unsolicited");
    assert!(urc.line.starts_with("+CMTI:"));
    simulator.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn cancelled_sim_switch_keeps_retries_atomic_and_drains_urcs_during_wait() {
    use crate::vendor::{self, Operation, Runtime};
    let device=serde_json::from_value(serde_json::json!({"id":"m1","name":"test","manufacturer":"quectel","platform":"qualcomm","at_port":"/dev/ttyUSB2","bus":"usb"})).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let program = vendor::plan(
        &device,
        &Operation::SetSimSlot { slot: 2 },
        &Runtime::new(dir.path()),
    )
    .unwrap();
    let (client, mut modem) = duplex(4096);
    let port = Port::start(client);
    let cloned = port.clone();
    let mut events = port.subscribe();
    let first = tokio::spawn(async move { cloned.run(program).await });
    async fn expect(modem: &mut tokio::io::DuplexStream, expected: &str) {
        let mut bytes = vec![0; expected.len()];
        modem.read_exact(&mut bytes).await.unwrap();
        assert_eq!(bytes, expected.as_bytes());
    }
    expect(&mut modem, "AT+QUIMSLOT=2\r\n").await;
    first.abort();
    let second = tokio::spawn(async move { port.execute(vec![command("AT")]).await });
    modem.write_all(b"OK\r\n").await.unwrap();
    let start = Instant::now();
    expect(&mut modem, "AT+QUIMSLOT?\r\n").await;
    assert_eq!(Instant::now(), start);
    modem
        .write_all(b"+QUIMSLOT: 1\r\nOK\r\n+CMTI: \"SM\",7\r\n")
        .await
        .unwrap();
    loop {
        let event = events.recv().await.unwrap();
        if event.line.starts_with("+CMTI:") {
            assert_eq!(event.correlation, "unsolicited");
            break;
        }
    }
    assert_eq!(Instant::now(), start);
    expect(&mut modem, "AT+QUIMSLOT?\r\n").await;
    assert_eq!(Instant::now() - start, Duration::from_secs(1));
    modem.write_all(b"+QUSIMSLOT: 2\r\nOK\r\n").await.unwrap();
    expect(&mut modem, "AT\r\n").await;
    modem.write_all(b"second\r\nOK\r\n").await.unwrap();
    assert!(
        second.await.unwrap().unwrap()[0]
            .response
            .contains("second")
    );
}

#[tokio::test]
async fn continue_on_modem_error_keeps_next_reply_and_trailing_urc_separate() {
    let (client, mut modem) = duplex(4096);
    let port = Port::start(client);
    let mut events = port.subscribe();
    let simulator = tokio::spawn(async move {
        let mut bytes = [0; 5];
        modem.read_exact(&mut bytes).await.unwrap();
        assert_eq!(&bytes, b"AT1\r\n");
        modem
            .write_all(b"ERROR\r\n+CMTI: \"SM\",3\r\n")
            .await
            .unwrap();
        modem.read_exact(&mut bytes).await.unwrap();
        assert_eq!(&bytes, b"AT2\r\n");
        modem.write_all(b"second\r\nOK\r\n").await.unwrap();
        sleep(Duration::from_millis(5)).await;
    });
    let r = port
        .run(Box::new(Sequence::new(
            vec![command("AT1"), command("AT2")],
            true,
        )))
        .await
        .unwrap();
    assert_eq!(r.len(), 2);
    assert!(!r[0].modem_success);
    assert!(r[1].modem_success);
    assert!(!r[1].response.contains("+CMTI:"));
    let mut urc = false;
    while let Ok(e) = events.try_recv() {
        if e.line.starts_with("+CMTI:") {
            urc = true;
            assert_eq!(e.correlation, "unsolicited");
        }
    }
    assert!(urc);
    simulator.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn queue_snapshot_tracks_capacity_cancellation_and_bounded_history_without_payloads() {
    let (client, mut modem) = duplex(4096);
    let port = Port::start(client);
    let cloned = port.clone();
    let first = tokio::spawn(async move {
        cloned
            .run_named(
                Box::new(Sequence::new(vec![command("AT+SECRET")], false)),
                Some("m1".into()),
                "raw_at",
            )
            .await
    });
    let mut bytes = [0; 11];
    modem.read_exact(&mut bytes).await.unwrap();
    let current = port.snapshot();
    assert_eq!(current.state, "running");
    assert_eq!(current.current.unwrap().modem_id.as_deref(), Some("m1"));
    let mut queued = Vec::new();
    for _ in 0..32 {
        let cloned = port.clone();
        queued.push(tokio::spawn(async move {
            cloned.execute(vec![command("AT")]).await
        }));
    }
    tokio::task::yield_now().await;
    assert_eq!(port.snapshot().waiting_count, 32);
    assert_eq!(
        port.execute(vec![command("AT")]).await.unwrap_err().kind,
        ErrorKind::QueueFull
    );
    assert_eq!(port.snapshot().rejected_queue_full, 1);
    queued.remove(0).abort();
    tokio::task::yield_now().await;
    modem.write_all(b"OK\r\n").await.unwrap();
    first.await.unwrap().unwrap();
    for _ in 0..31 {
        let mut b = [0; 4];
        modem.read_exact(&mut b).await.unwrap();
        assert_eq!(&b, b"AT\r\n");
        modem.write_all(b"OK\r\n").await.unwrap();
    }
    for request in queued {
        request.await.unwrap().unwrap();
    }
    let view = port.snapshot();
    assert_eq!(view.waiting_count, 0);
    assert_eq!(view.state, "idle");
    assert_eq!(view.cancelled_before_start, 1);
    assert_eq!(view.completed, 32);
    assert_eq!(view.recent.len(), 32);
    let serialized = serde_json::to_string(&view).unwrap();
    assert!(!serialized.contains("SECRET"));
    assert!(!serialized.contains("response"));
    drop(modem);
    tokio::task::yield_now().await;
    assert_eq!(port.snapshot().state, "closed");
}

#[tokio::test]
async fn aliases_of_one_native_device_share_queue_and_monitor() {
    use std::os::fd::AsRawFd;
    let pty = nix::pty::openpty(None, None).unwrap();
    let path = nix::unistd::ttyname(&pty.slave).unwrap();
    let path = path.to_str().unwrap();
    let alias = format!("/dev/fd/{}", pty.slave.as_raw_fd());
    let pool = PortPool::default();
    assert!(pool.inspect(path).await.is_none());
    let port = pool.get(path).await.unwrap();
    let aliased = pool.get(&alias).await.unwrap();
    assert!(port.sender.same_channel(&aliased.sender));
    assert_eq!(pool.ports.lock().await.len(), 1);
    let (canonical, view) = pool.inspect(&alias).await.unwrap();
    assert_eq!(canonical, path);
    assert_eq!(view.state, "idle");
}

#[tokio::test]
async fn close_rejects_active_transactions_and_stale_handles() {
    let (client, mut modem) = duplex(4096);
    let port = Port::start(client);
    let saved = port.clone();
    let handle = tokio::spawn(async move { saved.execute(vec![command("AT")]).await });
    let mut buf = [0; 4];
    modem.read_exact(&mut buf).await.unwrap();
    assert!(port.close().await.is_err());
    modem.write_all(b"OK\r\n").await.unwrap();
    handle.await.unwrap().unwrap();
    port.close().await.unwrap();
    assert_eq!(port.snapshot().state, "closed");
    assert_eq!(
        port.execute(vec![command("AT")]).await.unwrap_err().kind,
        ErrorKind::Closed
    );
    assert_eq!(modem.read(&mut buf).await.unwrap(), 0);
}
#[tokio::test]
async fn pool_reopens_after_close_without_reusing_old_handles() {
    let pty = nix::pty::openpty(None, None).unwrap();
    let path = nix::unistd::ttyname(&pty.slave).unwrap();
    drop(pty.slave);
    let path = path.to_str().unwrap();
    let pool = PortPool::default();
    let first = pool.get(path).await.unwrap();
    let mut events = first.subscribe();
    pool.close(path).await.unwrap();
    assert_eq!(events.recv().await.unwrap().correlation, "closed");
    let second = pool.get(path).await.unwrap();
    assert_eq!(second.snapshot().state, "idle");
    assert_eq!(
        first.execute(vec![command("AT")]).await.unwrap_err().kind,
        ErrorKind::Closed
    );
    pool.close(path).await.unwrap();
    drop(pty.master);
}

#[tokio::test]
async fn mt5700_notifications_are_separate_from_fragmented_command_response() {
    let (client, mut modem) = duplex(4096);
    let port = Port::start(client);
    let mut events = port.subscribe();
    let simulator = tokio::spawn(async move {
        let mut cmd = [0; 9];
        modem.read_exact(&mut cmd).await.unwrap();
        assert_eq!(&cmd, b"AT+CGMM\r\n");
        modem.write_all(b"^RSSI: 32\r\n^CER").await.unwrap();
        sleep(Duration::from_millis(5)).await;
        modem
            .write_all(b"SSI: 0,0,255\r\nMT5700M-CN\r\n+CMTI: \"SM\",7\r\n+C5GREG: 1\r\nOK\r\n")
            .await
            .unwrap();
        sleep(Duration::from_millis(10)).await;
    });
    let reply = port.execute(vec![command("AT+CGMM")]).await.unwrap();
    assert_eq!(reply[0].response, "MT5700M-CN\r\nOK\r\n");
    let mut unsolicited = Vec::new();
    while let Ok(event) = events.try_recv() {
        if event.correlation == "unsolicited" {
            unsolicited.push(event.line);
        }
    }
    assert_eq!(
        unsolicited,
        [
            "^RSSI: 32",
            "^CERSSI: 0,0,255",
            "+CMTI: \"SM\",7",
            "+C5GREG: 1"
        ]
    );
    simulator.await.unwrap();
}

#[tokio::test]
async fn solicited_notification_queries_and_unknown_lines_are_preserved() {
    let (client, mut modem) = duplex(4096);
    let port = Port::start(client);
    let simulator = tokio::spawn(async move {
        for (request, response) in [
            ("at^hcsq?\r\n", "^HCSQ: \"NR\",70,171,30\r\nOK\r\n"),
            ("AT+CEREG?\r\n", "+CEREG: 2,1\r\nOK\r\n"),
            ("AT+CPIN?\r\n", "+CPIN: READY\r\n^UNKNOWN: 7\r\nOK\r\n"),
        ] {
            let mut cmd = vec![0; request.len()];
            modem.read_exact(&mut cmd).await.unwrap();
            assert_eq!(cmd, request.as_bytes());
            modem.write_all(response.as_bytes()).await.unwrap();
        }
    });
    let replies = port
        .execute(vec![
            command("at^hcsq?"),
            command("AT+CEREG?"),
            command("AT+CPIN?"),
        ])
        .await
        .unwrap();
    assert!(replies[0].response.starts_with("^HCSQ:"));
    assert!(replies[1].response.starts_with("+CEREG:"));
    assert!(replies[2].response.starts_with("+CPIN:"));
    assert!(replies[2].response.contains("^UNKNOWN: 7"));
    simulator.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn quarantined_port_recovers_from_fragmented_late_terminal_without_replaying() {
    let (client, mut modem) = duplex(4096);
    let port = Port::start(client);
    let saved = port.clone();
    let first = tokio::spawn(async move {
        saved
            .execute(vec![
                Step::command("AT", Duration::from_millis(20)).unwrap(),
            ])
            .await
    });
    let mut bytes = [0; 4];
    modem.read_exact(&mut bytes).await.unwrap();
    tokio::time::advance(Duration::from_millis(21)).await;
    assert_eq!(first.await.unwrap().unwrap_err().kind, ErrorKind::Timeout);
    modem.write_all(b"late\r\nO").await.unwrap();
    tokio::task::yield_now().await;
    tokio::time::advance(RECOVERY_TIMEOUT + Duration::from_millis(1)).await;
    tokio::task::yield_now().await;
    assert_eq!(port.snapshot().state, "quarantined");
    assert_eq!(
        port.execute(vec![command("AT")]).await.unwrap_err().kind,
        ErrorKind::Unsynchronized
    );
    modem.write_all(b"K\r\n+CMTI: \"SM\",1\r\n").await.unwrap();
    tokio::task::yield_now().await;
    tokio::time::advance(QUIET + Duration::from_millis(1)).await;
    tokio::task::yield_now().await;
    assert_eq!(port.snapshot().state, "idle");
    let saved = port.clone();
    let second = tokio::spawn(async move { saved.execute(vec![command("AT2")]).await });
    let mut bytes = [0; 5];
    modem.read_exact(&mut bytes).await.unwrap();
    assert_eq!(&bytes, b"AT2\r\n");
    modem.write_all(b"fresh\r\nOK\r\n").await.unwrap();
    let result = second.await.unwrap().unwrap();
    assert_eq!(result[0].response, "fresh\r\nOK\r\n");
}

#[tokio::test(start_paused = true)]
async fn late_sms_prompt_does_not_make_port_synchronized() {
    let (client, mut modem) = duplex(4096);
    let port = Port::start(client);
    let saved = port.clone();
    let first = tokio::spawn(async move {
        let mut prompt = Step::command("AT+CMGS=4", Duration::from_millis(20)).unwrap();
        prompt.flags = vec![">".into(), "ERROR".into(), "+CMS ERROR:".into()];
        saved.execute(vec![prompt]).await
    });
    let mut bytes = [0; 11];
    modem.read_exact(&mut bytes).await.unwrap();
    tokio::time::advance(Duration::from_millis(21)).await;
    assert_eq!(first.await.unwrap().unwrap_err().kind, ErrorKind::Timeout);
    modem.write_all(b"\r\n> ").await.unwrap();
    tokio::task::yield_now().await;
    tokio::time::advance(RECOVERY_TIMEOUT + Duration::from_millis(1)).await;
    tokio::task::yield_now().await;
    assert_eq!(port.snapshot().state, "quarantined");
    assert_eq!(
        port.execute(vec![command("AT")]).await.unwrap_err().kind,
        ErrorKind::Unsynchronized
    );
}

#[test]
fn command_metadata_never_contains_arguments_or_arbitrary_verbs() {
    assert_eq!(command_label(b"AT+CPIN=\"1234\"\r\n"), "AT+CPIN");
    assert_eq!(
        command_label(b"AT^SETAUTODIAL=1,1,\"IP\",\"secret\"\r\n"),
        "AT^SETAUTODIAL"
    );
    assert_eq!(command_label(b"AT+SECRET=private\r\n"), "AT / payload");
    assert_eq!(command_label(b"private SMS body\x1a"), "AT / payload");
}
