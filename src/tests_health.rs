//! 健康探测的真实网络层：本地**假 HTTP sidecar** + 状态行解析。
//!
//! 这一层不注入 [`Prober`]——它就是要证明「真连一个 TCP 端口、真发
//! `GET /healthz`、真解析状态行」是通的。

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use crate::health::{http_health, parse_status_code, port_accepts};
use crate::tests_support::free_port;

/// 起一个只应答一次连接、把请求原文记下来的假 sidecar。
fn fake_sidecar(response: &'static str) -> (u16, Arc<Mutex<Vec<String>>>, JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind 假 sidecar");
    let port = listener.local_addr().expect("本机地址").port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen_by_thread = Arc::clone(&seen);
    let handle = std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let mut buf = [0u8; 2048];
        let read = stream.read(&mut buf).unwrap_or(0);
        seen_by_thread
            .lock()
            .unwrap()
            .push(String::from_utf8_lossy(&buf[..read]).into_owned());
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.flush();
    });
    (port, seen, handle)
}

#[test]
fn health_ok_against_real_http_sidecar() {
    let (port, seen, handle) = fake_sidecar(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 15\r\nConnection: close\r\n\r\n{\"status\":\"ok\"}",
    );
    assert!(
        http_health(port, std::time::Duration::from_secs(2)),
        "2xx 应判就绪"
    );
    handle.join().expect("假 sidecar 线程");
    let requests = seen.lock().unwrap();
    let request = requests.first().expect("应收到一次请求");
    assert!(
        request.starts_with("GET /healthz HTTP/1.1"),
        "必须按契约探 /healthz：{request:?}"
    );
    assert!(
        request.contains("Host: 127.0.0.1"),
        "应带 Host 头：{request:?}"
    );
}

#[test]
fn health_rejects_non_2xx() {
    for response in [
        "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\n\r\n",
        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n",
        "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\r\n",
        "HTTP/1.1 302 Found\r\nLocation: /\r\n\r\n",
    ] {
        let (port, _seen, handle) = fake_sidecar(response);
        assert!(
            !http_health(port, std::time::Duration::from_secs(2)),
            "非 2xx 不能判就绪：{response:?}"
        );
        handle.join().expect("假 sidecar 线程");
    }
}

#[test]
fn health_rejects_garbage_response() {
    let (port, _seen, handle) = fake_sidecar("not-http at all\r\n\r\n");
    assert!(!http_health(port, std::time::Duration::from_secs(2)));
    handle.join().expect("假 sidecar 线程");
}

#[test]
fn health_is_false_when_nothing_listens() {
    let port = free_port();
    assert!(
        !http_health(port, std::time::Duration::from_millis(300)),
        "没人监听 → 探活 false（不是 Err、不是 panic）"
    );
}

#[test]
fn port_accepts_sees_a_listener() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind");
    let port = listener.local_addr().expect("本机地址").port();
    // 监听队列里没人 accept 也能连上（backlog），这正是「端口被占」的判据。
    assert!(port_accepts(port, std::time::Duration::from_millis(300)));
    drop(listener);
    let port = free_port();
    assert!(!port_accepts(port, std::time::Duration::from_millis(300)));
}

#[test]
fn status_line_parser() {
    assert_eq!(parse_status_code("HTTP/1.1 200 OK\r\n"), Some(200));
    assert_eq!(parse_status_code("HTTP/1.0 204 No Content"), Some(204));
    assert_eq!(
        parse_status_code("HTTP/1.1 503 Service Unavailable"),
        Some(503)
    );
    // 不合法：非 HTTP 首行 / 缺状态码 / 状态码非数字 / 空
    assert_eq!(parse_status_code(""), None);
    assert_eq!(parse_status_code("PING"), None);
    assert_eq!(parse_status_code("HTTP/1.1"), None);
    assert_eq!(parse_status_code("HTTP/1.1 abc OK"), None);
    // 响应头里夹杂内容也不影响（只看第一行）
    assert_eq!(
        parse_status_code("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}"),
        Some(200)
    );
}
