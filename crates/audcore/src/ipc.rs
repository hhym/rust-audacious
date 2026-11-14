//! audtool ↔ 播放器之间的控制协议。
//!
//! 行分隔 JSON，跑在 127.0.0.1 某个随机端口上，端口号写进
//! `<config>/audacious-rs/control.port`，等价于 Audacious 里 audtool
//! 通过本地 socket 控制 GUI 的做法（这里换成 TCP，跨平台实现更简单）。

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Request {
    pub cmd: String,
    #[serde(default)]
    pub args: Vec<Value>,
}

impl Request {
    pub fn new(cmd: &str, args: Vec<Value>) -> Self {
        Request {
            cmd: cmd.into(),
            args,
        }
    }

    pub fn arg_str(&self, index: usize) -> Option<&str> {
        self.args.get(index).and_then(|v| v.as_str())
    }

    pub fn arg_u64(&self, index: usize) -> Option<u64> {
        self.args.get(index).and_then(|v| v.as_u64())
    }

    pub fn arg_f64(&self, index: usize) -> Option<f64> {
        self.args.get(index).and_then(|v| v.as_f64())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    #[serde(default)]
    pub data: Value,
    #[serde(default)]
    pub error: Option<String>,
}

impl Response {
    pub fn ok(data: Value) -> Self {
        Response {
            ok: true,
            data,
            error: None,
        }
    }

    pub fn empty() -> Self {
        Response::ok(Value::Null)
    }

    pub fn err(message: impl Into<String>) -> Self {
        Response {
            ok: false,
            data: Value::Null,
            error: Some(message.into()),
        }
    }
}

/// 端口文件路径。
pub fn port_file() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(crate::APP_NAME)
        .join("control.port")
}

/// 从端口文件读取服务端口；没有则回退到默认 6600。
pub fn read_port() -> u16 {
    std::fs::read_to_string(port_file())
        .ok()
        .and_then(|s| s.trim().parse::<u16>().ok())
        .unwrap_or(DEFAULT_PORT)
}

pub const DEFAULT_PORT: u16 = 6600;

fn connect(port: u16) -> Result<TcpStream> {
    let addr = ("127.0.0.1", port)
        .to_socket_addrs()?
        .next()
        .context("解析控制端口失败")?;
    let stream = TcpStream::connect_timeout(&addr, Duration::from_millis(800))
        .context("连接播放器失败（播放器没在运行？）")?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.set_write_timeout(Some(Duration::from_secs(3)))?;
    Ok(stream)
}

/// 同步调用一次控制命令。
pub fn call(request: Request) -> Result<Response> {
    let port = read_port();
    let mut stream = connect(port)?;
    let line = serde_json::to_string(&request)?;
    stream.write_all(line.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;

    let mut reader = BufReader::new(stream);
    let mut buf = String::new();
    reader.read_line(&mut buf)?;
    if buf.trim().is_empty() {
        bail!("播放器没有返回响应");
    }
    Ok(serde_json::from_str(buf.trim())?)
}
