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
