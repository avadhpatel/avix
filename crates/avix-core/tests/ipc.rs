use avix_core::ipc::{frame, message::*};
use serde_json::json;

#[test]
fn frame_encode_decode_round_trip() {
    let payload = json!({"jsonrpc": "2.0", "id": "1", "method": "fs/read", "params": {}});
    let bytes = frame::encode(&payload).unwrap();
    let len = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
    assert_eq!(len, bytes.len() - 4);
    let decoded: serde_json::Value = frame::decode(&bytes).unwrap();
    assert_eq!(decoded, payload);
}

#[test]
fn frame_length_prefix_is_little_endian() {
    let payload = json!({"x": 1});
    let bytes = frame::encode(&payload).unwrap();
    let le_len = u32::from_le_bytes(bytes[..4].try_into().unwrap());
    assert_eq!(le_len as usize, bytes.len() - 4);
}

#[test]
fn frame_rejects_oversized_message() {
    let big_string = "x".repeat(17 * 1024 * 1024);
    let payload = json!({"data": big_string});
    assert!(frame::encode(&payload).is_err());
}

#[test]
fn jsonrpc_request_serialises() {
    let req = JsonRpcRequest {
        jsonrpc: "2.0".into(),
        id: "abc-123".into(),
        method: "fs/read".into(),
        params: json!({"path": "/etc/avix/kernel.yaml"}),
    };
    let v = serde_json::to_value(&req).unwrap();
    assert_eq!(v["jsonrpc"], "2.0");
    assert_eq!(v["method"], "fs/read");
    assert_eq!(v["id"], "abc-123");
}

#[test]
fn jsonrpc_response_ok_serialises() {
    let resp = JsonRpcResponse::ok("abc-123", json!({"content": "hello"}));
    let v = serde_json::to_value(&resp).unwrap();
    assert_eq!(v["result"]["content"], "hello");
    assert!(v.get("error").is_none() || v["error"].is_null());
}

#[test]
fn jsonrpc_response_err_serialises() {
    let resp = JsonRpcResponse::err("abc-123", -32001, "EAUTH", None);
    let v = serde_json::to_value(&resp).unwrap();
    assert_eq!(v["error"]["code"], -32001);
    assert_eq!(v["error"]["message"], "EAUTH");
    assert!(v.get("result").is_none() || v["result"].is_null());
}

#[test]
fn jsonrpc_error_codes_defined() {
    assert_eq!(JsonRpcErrorCode::Eauth as i32, -32001);
    assert_eq!(JsonRpcErrorCode::Eperm as i32, -32002);
    assert_eq!(JsonRpcErrorCode::Enoent as i32, -32003);
    assert_eq!(JsonRpcErrorCode::Ebusy as i32, -32004);
    assert_eq!(JsonRpcErrorCode::Etimeout as i32, -32005);
    assert_eq!(JsonRpcErrorCode::Eused as i32, -32009);
}

#[tokio::test]
async fn ipc_round_trip_over_socket() {
    use avix_core::ipc::transport::test_socket_pair;
    use tokio::io::AsyncWriteExt;

    let (mut client, mut server) = test_socket_pair().await;
    let req = JsonRpcRequest {
        jsonrpc: "2.0".into(),
        id: "t1".into(),
        method: "ping".into(),
        params: json!({}),
    };
    let bytes = frame::encode(&req).unwrap();
    client.write_all(&bytes).await.unwrap();
    let received: JsonRpcRequest = frame::read_from(&mut server).await.unwrap();
    assert_eq!(received.method, "ping");
    assert_eq!(received.id, "t1");
}

#[tokio::test]
async fn ipc_fresh_connection_per_call_model() {
    use avix_core::ipc::transport::test_socket_pair;
    use tokio::io::AsyncWriteExt;
    let (mut c1, _s1) = test_socket_pair().await;
    let (mut c2, _s2) = test_socket_pair().await;
    let req = frame::encode(&json!({"jsonrpc":"2.0","id":"1","method":"a","params":{}})).unwrap();
    c1.write_all(&req).await.unwrap();
    c2.write_all(&req).await.unwrap();
}
