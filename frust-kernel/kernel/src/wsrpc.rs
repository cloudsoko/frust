//! Minimal WebSocket RPC client for SurrealDB's `/rpc` endpoint.
//!
//! Hand-rolled RFC 6455 client over `TcpStream` — the kernel's zero-new-crates
//! posture. Scope: text frames, ping/pong, close; no fragmentation (SurrealDB
//! sends whole JSON messages); no TLS (loopback surreal, same trust domain as
//! every other kernel connection). The server's `Sec-WebSocket-Accept` is not
//! verified for the same reason.
//!
//! One connection per live subscription: `authenticate` with the SUBSCRIBER'S
//! OWN record JWT, `use`, then `LIVE SELECT` — the DB enforces the row clause
//! per subscriber; the kernel never filters the push path.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use crate::contract::BrokerError;

fn db_err(detail: impl Into<String>) -> BrokerError {
    BrokerError::Db { detail: detail.into() }
}

/// Correlation-grade entropy for handshake keys and frame masks.
fn entropy(n: u64) -> [u8; 4] {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    let x = nanos ^ (u64::from(std::process::id()) << 17) ^ n.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x.to_le_bytes()[..4].try_into().unwrap()
}

fn write_frame(stream: &mut TcpStream, opcode: u8, payload: &[u8], seq: u64) -> Result<(), BrokerError> {
    let mut frame = Vec::with_capacity(payload.len() + 14);
    frame.push(0x80 | opcode); // FIN + opcode
    let mask = entropy(seq);
    if payload.len() < 126 {
        frame.push(0x80 | payload.len() as u8);
    } else if payload.len() <= u16::MAX as usize {
        frame.push(0x80 | 126);
        frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    } else {
        frame.push(0x80 | 127);
        frame.extend_from_slice(&(payload.len() as u64).to_be_bytes());
    }
    frame.extend_from_slice(&mask);
    frame.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    stream.write_all(&frame).map_err(|e| db_err(format!("ws write: {e}")))
}

fn read_exact(stream: &mut TcpStream, buf: &mut [u8]) -> Result<(), BrokerError> {
    stream.read_exact(buf).map_err(|e| db_err(format!("ws read: {e}")))
}

/// Reads one complete frame; answers pings inline. Returns (opcode, payload).
fn read_frame(stream: &mut TcpStream, seq: &AtomicU64) -> Result<(u8, Vec<u8>), BrokerError> {
    let mut head = [0u8; 2];
    read_exact(stream, &mut head)?;
    let opcode = head[0] & 0x0F;
    let masked = head[1] & 0x80 != 0;
    let mut len = u64::from(head[1] & 0x7F);
    if len == 126 {
        let mut ext = [0u8; 2];
        read_exact(stream, &mut ext)?;
        len = u64::from(u16::from_be_bytes(ext));
    } else if len == 127 {
        let mut ext = [0u8; 8];
        read_exact(stream, &mut ext)?;
        len = u64::from_be_bytes(ext);
    }
    let mut mask = [0u8; 4];
    if masked {
        read_exact(stream, &mut mask)?;
    }
    let mut payload = vec![0u8; len as usize];
    read_exact(stream, &mut payload)?;
    if masked {
        for (i, b) in payload.iter_mut().enumerate() {
            *b ^= mask[i % 4];
        }
    }
    match opcode {
        0x9 => {
            // ping -> pong with same payload
            let n = seq.fetch_add(1, Ordering::Relaxed);
            let mut s = stream.try_clone().map_err(|e| db_err(format!("ws clone: {e}")))?;
            write_frame(&mut s, 0xA, &payload, n)?;
            read_frame(stream, seq)
        }
        0x8 => Err(db_err("ws closed by server")),
        0x0 => Err(db_err("ws fragmentation unsupported")),
        _ => Ok((opcode, payload)),
    }
}

/// A live RPC connection: sequential `call`s plus a notification callback
/// driven by a reader thread.
pub struct WsRpc {
    writer: Mutex<TcpStream>,
    seq: Arc<AtomicU64>,
    next_id: AtomicU64,
    pending: Arc<Mutex<std::collections::HashMap<String, mpsc::Sender<serde_json::Value>>>>,
    alive: Arc<AtomicBool>,
}

impl WsRpc {
    /// Connects, upgrades, and starts the reader thread. `on_notify` receives
    /// every non-response message (live notifications).
    pub fn connect(
        addr: &str,
        on_notify: impl Fn(serde_json::Value) + Send + 'static,
    ) -> Result<Self, BrokerError> {
        let mut stream = TcpStream::connect(addr).map_err(|e| db_err(format!("ws connect {addr}: {e}")))?;
        stream
            .set_read_timeout(Some(Duration::from_secs(3600)))
            .map_err(|e| db_err(format!("ws timeout: {e}")))?;

        // RFC 6455 client handshake (accept-key unverified: loopback trust)
        let mut key = [0u8; 16];
        for (i, chunk) in key.chunks_mut(4).enumerate() {
            chunk.copy_from_slice(&entropy(i as u64 + 1));
        }
        let key64 = b64(&key);
        let host = addr;
        let req = format!(
            "GET /rpc HTTP/1.1\r\nHost: {host}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
             Sec-WebSocket-Key: {key64}\r\nSec-WebSocket-Version: 13\r\n\r\n"
        );
        stream.write_all(req.as_bytes()).map_err(|e| db_err(format!("ws handshake write: {e}")))?;
        let mut response = Vec::new();
        let mut byte = [0u8; 1];
        while !response.ends_with(b"\r\n\r\n") {
            read_exact(&mut stream, &mut byte)?;
            response.push(byte[0]);
            if response.len() > 8192 {
                return Err(db_err("ws handshake response too large"));
            }
        }
        if !response.starts_with(b"HTTP/1.1 101") {
            return Err(db_err(format!(
                "ws upgrade refused: {}",
                String::from_utf8_lossy(&response[..response.len().min(120)])
            )));
        }

        let seq = Arc::new(AtomicU64::new(1));
        let pending: Arc<Mutex<std::collections::HashMap<String, mpsc::Sender<serde_json::Value>>>> =
            Arc::new(Mutex::new(std::collections::HashMap::new()));
        let alive = Arc::new(AtomicBool::new(true));

        let mut reader = stream.try_clone().map_err(|e| db_err(format!("ws clone: {e}")))?;
        {
            let pending = Arc::clone(&pending);
            let alive = Arc::clone(&alive);
            let seq = Arc::clone(&seq);
            std::thread::spawn(move || {
                loop {
                    match read_frame(&mut reader, &seq) {
                        Ok((0x1, payload)) => {
                            let Ok(msg) = serde_json::from_slice::<serde_json::Value>(&payload) else {
                                continue;
                            };
                            let id = msg.get("id").and_then(|i| i.as_str()).map(str::to_string);
                            match id.and_then(|i| pending.lock().unwrap().remove(&i)) {
                                Some(tx) => {
                                    let _ = tx.send(msg);
                                }
                                None => on_notify(msg),
                            }
                        }
                        Ok(_) => {} // binary/pong: ignore
                        Err(_) => {
                            alive.store(false, Ordering::Relaxed);
                            break;
                        }
                    }
                }
            });
        }

        Ok(Self {
            writer: Mutex::new(stream),
            seq,
            next_id: AtomicU64::new(1),
            pending,
            alive,
        })
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }

    /// One RPC round-trip. 10 s deadline: a hung call is a dead subscription.
    pub fn call(&self, method: &str, params: serde_json::Value) -> Result<serde_json::Value, BrokerError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed).to_string();
        let (tx, rx) = mpsc::channel();
        self.pending.lock().unwrap().insert(id.clone(), tx);
        let msg = serde_json::json!({ "id": id, "method": method, "params": params });
        {
            let mut w = self.writer.lock().unwrap();
            let n = self.seq.fetch_add(1, Ordering::Relaxed);
            write_frame(&mut w, 0x1, msg.to_string().as_bytes(), n)?;
        }
        let reply = rx
            .recv_timeout(Duration::from_secs(10))
            .map_err(|_| db_err(format!("ws rpc timeout: {method}")))?;
        if let Some(err) = reply.get("error").filter(|e| !e.is_null()) {
            return Err(db_err(format!("ws rpc {method}: {err}")));
        }
        Ok(reply.get("result").cloned().unwrap_or(serde_json::Value::Null))
    }

    pub fn close(&self) {
        self.alive.store(false, Ordering::Relaxed);
        if let Ok(mut w) = self.writer.lock() {
            let n = self.seq.fetch_add(1, Ordering::Relaxed);
            let _ = write_frame(&mut w, 0x8, &[], n);
            let _ = w.shutdown(std::net::Shutdown::Both);
        }
    }
}

/// Standard base64 (the db.rs alphabet, shared shape).
fn b64(bytes: &[u8]) -> String {
    const AL: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        out.push(AL[(n >> 18) as usize & 63] as char);
        out.push(AL[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { AL[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { AL[n as usize & 63] as char } else { '=' });
    }
    out
}
