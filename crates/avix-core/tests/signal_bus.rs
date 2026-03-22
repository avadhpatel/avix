use avix_core::signal::{Signal, SignalBus, SignalKind};
use avix_core::types::Pid;
use std::sync::Arc;
use std::time::Duration;

fn sigpause(pid: u32) -> Signal {
    Signal {
        target: Pid::new(pid),
        kind: SignalKind::Pause,
        payload: serde_json::Value::Null,
    }
}

fn sigresume(pid: u32, payload: serde_json::Value) -> Signal {
    Signal {
        target: Pid::new(pid),
        kind: SignalKind::Resume,
        payload,
    }
}

#[tokio::test]
async fn subscribe_and_receive_signal() {
    let bus = SignalBus::new();
    let mut rx = bus.subscribe(Pid::new(57)).await;
    bus.send(sigpause(57)).await.unwrap();
    let sig = tokio::time::timeout(Duration::from_millis(100), rx.recv())
        .await
        .expect("timeout")
        .expect("channel closed");
    assert_eq!(sig.kind, SignalKind::Pause);
    assert_eq!(sig.target, Pid::new(57));
}

#[tokio::test]
async fn multiple_subscribers_all_receive() {
    let bus = SignalBus::new();
    let mut rx1 = bus.subscribe(Pid::new(57)).await;
    let mut rx2 = bus.subscribe(Pid::new(57)).await;
    bus.send(sigpause(57)).await.unwrap();
    let s1 = tokio::time::timeout(Duration::from_millis(100), rx1.recv())
        .await
        .unwrap()
        .unwrap();
    let s2 = tokio::time::timeout(Duration::from_millis(100), rx2.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(s1.kind, SignalKind::Pause);
    assert_eq!(s2.kind, SignalKind::Pause);
}

#[tokio::test]
async fn signal_not_delivered_to_wrong_pid() {
    let bus = SignalBus::new();
    let mut rx_57 = bus.subscribe(Pid::new(57)).await;
    let mut rx_58 = bus.subscribe(Pid::new(58)).await;
    bus.send(sigpause(57)).await.unwrap();
    let s = tokio::time::timeout(Duration::from_millis(100), rx_57.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(s.kind, SignalKind::Pause);
    let nothing = tokio::time::timeout(Duration::from_millis(50), rx_58.recv()).await;
    assert!(
        nothing.is_err(),
        "PID 58 should not have received the signal"
    );
}

#[tokio::test]
async fn sigresume_carries_payload() {
    let bus = SignalBus::new();
    let mut rx = bus.subscribe(Pid::new(57)).await;
    let payload = serde_json::json!({ "hilId": "hil-001", "decision": "approved" });
    bus.send(sigresume(57, payload.clone())).await.unwrap();
    let sig = tokio::time::timeout(Duration::from_millis(100), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(sig.kind, SignalKind::Resume);
    assert_eq!(sig.payload["hilId"], "hil-001");
    assert_eq!(sig.payload["decision"], "approved");
}

#[tokio::test]
async fn broadcast_reaches_all_subscribers() {
    let bus = SignalBus::new();
    let mut rx57 = bus.subscribe(Pid::new(57)).await;
    let mut rx58 = bus.subscribe(Pid::new(58)).await;
    let mut rx59 = bus.subscribe(Pid::new(59)).await;
    bus.broadcast(SignalKind::Kill, serde_json::Value::Null)
        .await;
    for rx in [&mut rx57, &mut rx58, &mut rx59] {
        let s = tokio::time::timeout(Duration::from_millis(100), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(s.kind, SignalKind::Kill);
    }
}

#[tokio::test]
async fn unsubscribe_stops_delivery() {
    let bus = Arc::new(SignalBus::new());
    let rx = bus.subscribe(Pid::new(57)).await;
    let id = rx.id();
    bus.unsubscribe(Pid::new(57), id).await;
    bus.send(sigpause(57)).await.unwrap();
    assert_eq!(bus.subscriber_count(Pid::new(57)).await, 0);
}

#[tokio::test]
async fn send_to_unsubscribed_pid_is_noop() {
    let bus = SignalBus::new();
    bus.send(sigpause(99)).await.unwrap();
}

#[test]
fn signal_kind_names() {
    assert_eq!(SignalKind::Pause.as_str(), "SIGPAUSE");
    assert_eq!(SignalKind::Resume.as_str(), "SIGRESUME");
    assert_eq!(SignalKind::Kill.as_str(), "SIGKILL");
    assert_eq!(SignalKind::Stop.as_str(), "SIGSTOP");
    assert_eq!(SignalKind::Save.as_str(), "SIGSAVE");
    assert_eq!(SignalKind::Escalate.as_str(), "SIGESCALATE");
    assert_eq!(SignalKind::Start.as_str(), "SIGSTART");
    assert_eq!(SignalKind::Pipe.as_str(), "SIGPIPE");
}

#[tokio::test]
async fn concurrent_sends_all_received() {
    let bus = Arc::new(SignalBus::new());
    let mut rx = bus.subscribe(Pid::new(57)).await;
    let mut senders = Vec::new();
    for _ in 0..20 {
        let b = Arc::clone(&bus);
        senders.push(tokio::spawn(async move {
            b.send(sigpause(57)).await.unwrap();
        }));
    }
    for s in senders {
        s.await.unwrap();
    }
    let mut count = 0;
    while tokio::time::timeout(Duration::from_millis(50), rx.recv())
        .await
        .is_ok()
    {
        count += 1;
        if count == 20 {
            break;
        }
    }
    assert_eq!(count, 20);
}
