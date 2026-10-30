//! The fake HA's wire handling: read one HTTP request, record it as a
//! [`Hit`], answer according to the current [`HaMode`].

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::fake_ha::{HaMode, Hit};

/// How long the fake waits for a client to send its request bytes.
const READ_TIMEOUT: Duration = Duration::from_secs(30);

/// Read buffer size for the request head and body.
const READ_CHUNK: usize = 4096;

/// End of the HTTP head.
const HEAD_TERMINATOR: &[u8] = b"\r\n\r\n";

pub(super) fn handle(mut stream: TcpStream, hits: Arc<Mutex<Vec<Hit>>>, mode: HaMode) {
    stream
        .set_read_timeout(Some(READ_TIMEOUT))
        .expect("read timeout");
    let Some((raw, header_end)) = read_head(&mut stream) else {
        return;
    };
    let hit = parse_hit(&raw, header_end, &mut stream);
    hits.lock().unwrap().push(hit);
    // The client may already have hung up (kill drills); the hit is
    // recorded either way.
    let _ = stream.write_all(response_for(mode).as_bytes());
}

/// Read until the end of the HTTP head; returns the bytes read so far
/// and the head's length, or None if the client went away first.
fn read_head(stream: &mut TcpStream) -> Option<(Vec<u8>, usize)> {
    let mut raw = Vec::new();
    let mut buffer = [0u8; READ_CHUNK];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) | Err(_) => return None,
            Ok(read) => {
                raw.extend_from_slice(&buffer[..read]);
                if let Some(position) = find_header_end(&raw) {
                    return Some((raw, position));
                }
            }
        }
    }
}

/// Turn the head (and as much body as `content-length` announces) into
/// a recorded [`Hit`].
fn parse_hit(raw: &[u8], header_end: usize, stream: &mut TcpStream) -> Hit {
    let head = String::from_utf8_lossy(&raw[..header_end]).to_string();
    let mut lines = head.lines();
    let mut parts = lines.next().unwrap_or_default().split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();
    let mut content_type = None;
    let mut content_length = 0usize;
    let mut headers = Vec::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            let name = name.trim().to_ascii_lowercase();
            let value = value.trim().to_string();
            if name == "content-type" {
                content_type = Some(value.clone());
            }
            if name == "content-length" {
                content_length = value.parse().unwrap_or(0);
            }
            headers.push((name, value));
        }
    }
    let body = read_body(raw, header_end, content_length, stream);
    Hit {
        method,
        path,
        content_type,
        headers,
        body,
    }
}

fn read_body(raw: &[u8], header_end: usize, length: usize, stream: &mut TcpStream) -> Vec<u8> {
    let mut body = raw[header_end + HEAD_TERMINATOR.len()..].to_vec();
    let mut buffer = [0u8; READ_CHUNK];
    while body.len() < length {
        match stream.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(read) => body.extend_from_slice(&buffer[..read]),
        }
    }
    body
}

fn response_for(mode: HaMode) -> String {
    match mode {
        HaMode::Ok200 => {
            "HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_string()
        }
        HaMode::Fail(status) => {
            format!("HTTP/1.1 {status} NO\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
        }
        HaMode::Redirect => "HTTP/1.1 302 Found\r\nlocation: http://127.0.0.1:1/elsewhere\r\n\
             content-length: 0\r\nconnection: close\r\n\r\n"
            .to_string(),
        HaMode::SlowOk(delay) => {
            std::thread::sleep(delay);
            "HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_string()
        }
    }
}

fn find_header_end(raw: &[u8]) -> Option<usize> {
    raw.windows(HEAD_TERMINATOR.len())
        .position(|window| window == HEAD_TERMINATOR)
}
