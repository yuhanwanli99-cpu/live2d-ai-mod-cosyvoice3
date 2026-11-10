//! 探活：**最小 HTTP/1.1 客户端**（只为了 `GET /healthz`），零额外依赖。
//!
//! # 为什么自己写而不是拉一个 HTTP 客户端
//!
//! 目标只有「本机 127.0.0.1 的一个 GET，取状态码」：没有 TLS、没有重定向、没有
//! 连接池。为一个布尔值引入 HTTP 客户端会把这个 Mod 的依赖面（以及 Windows
//! 交叉编译面）撑大，而它换来的能力一条都用不上。
//!
//! # 语义（口径逐条）
//!
//! - **2xx 才算就绪**；3xx / 4xx / 5xx / 连不上 / 读到一半断开 → 未就绪；
//! - 单次超时由调用方给（生产 = [`crate::config::PROBE_TIMEOUT`] = 2 秒）；
//! - 只读响应头（最多 [`HEAD_MAX`] 字节），**不看 body**——body 是给人看的；
//! - 探活**不写任何东西**、不改 `[tts]`、不决定向哪里发合成请求（那是核心链路）。
//!
//! # 可注入
//!
//! [`Prober`] 是探活函数类型：生产用 [`http_health`]，单测注入假实现
//! （「第一次 false、之后 true」这类脚本），因此**不需要真的起 sidecar**。

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use crate::config::HEALTH_PATH;

/// 一次探活最多读多少字节的响应头。
pub const HEAD_MAX: usize = 8 * 1024;

/// 探活函数类型（单测注入假实现；生产 = [`http_health`]）。
pub type Prober = Box<dyn Fn(u16, Duration) -> bool + Send>;

/// `GET http://127.0.0.1:<port>/healthz` 是否 2xx。
///
/// **任何错误都返回 `false`**（连不上 / 超时 / 非法响应）：探活只回答
/// 「现在能不能服务」，不负责解释为什么不能——归因由调用方的状态机做
/// （`program_missing` / `port_in_use` / `health_timeout` / `spawn_failed`）。
pub fn http_health(port: u16, timeout: Duration) -> bool {
    let Some(head) = request_health_head(port, timeout) else {
        return false;
    };
    parse_status_code(&head).is_some_and(|code| (200..300).contains(&code))
}

/// 端口上有没有东西在**应答 TCP**（不管它健不健康）。
///
/// 用途只有一个：探活失败之后、spawn 之前，判断「端口是不是被**别的**进程占着」。
/// 占着就报 `port_in_use`，**不硬起第二个**（否则 `start.cmd` 会起一个绑不上端口
/// 又立刻退出的进程，用户看到的是 `spawn_failed`，与真实原因差一层）。
pub fn port_accepts(port: u16, timeout: Duration) -> bool {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    TcpStream::connect_timeout(&addr, timeout).is_ok()
}

/// 发一次探活请求并把响应头读回来（失败 = `None`）。
fn request_health_head(port: u16, timeout: Duration) -> Option<String> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = TcpStream::connect_timeout(&addr, timeout).ok()?;
    stream.set_read_timeout(Some(timeout)).ok()?;
    stream.set_write_timeout(Some(timeout)).ok()?;
    let request = format!(
        "GET {HEALTH_PATH} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAccept: */*\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).ok()?;
    let mut buf = [0u8; 1024];
    let mut head: Vec<u8> = Vec::new();
    loop {
        match stream.read(&mut buf) {
            // 对端 close/shutdown：手上有多少算多少。
            Ok(0) => break,
            Ok(n) => {
                head.extend_from_slice(&buf[..n]);
                // 响应头结束：状态行已完整，可以不再读 body。
                if head.windows(4).any(|w| w == b"\r\n\r\n") || head.len() >= HEAD_MAX {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    if head.is_empty() {
        None
    } else {
        Some(String::from_utf8_lossy(&head).into_owned())
    }
}

/// 从响应头里取状态码（`"HTTP/1.1 200 OK"` → `Some(200)`）。
///
/// 纯函数，单测直接喂字符串：非 HTTP 首行、缺状态码、状态码不是数字 → `None`。
pub fn parse_status_code(head: &str) -> Option<u16> {
    let line = head.lines().next()?;
    let mut parts = line.split_whitespace();
    let protocol = parts.next()?;
    if !protocol.starts_with("HTTP/") {
        return None;
    }
    parts.next()?.parse::<u16>().ok()
}
