#![allow(dead_code)] // each test binary uses a different part of this module
//! A tiny HTTP/1.1 server for host tests. It records every request and answers
//! with what the test's handler returns.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;

#[derive(Debug, Clone)]
pub struct Recorded {
    pub method: String,
    /// Path and query, as sent.
    pub target: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Recorded {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

pub struct FakeHost {
    /// `http://127.0.0.1:<port>`
    pub base: String,
    pub requests: Arc<Mutex<Vec<Recorded>>>,
}

impl FakeHost {
    pub fn recorded(&self) -> Vec<Recorded> {
        self.requests.lock().unwrap().clone()
    }
}

/// Starts a server on a free port. `handler` returns a status and a JSON body.
pub fn serve<F>(handler: F) -> FakeHost
where
    F: Fn(&Recorded) -> (u16, String) + Send + Sync + 'static,
{
    serve_with_headers(move |req| {
        let (status, body) = handler(req);
        (status, Vec::new(), body)
    })
}

/// Like `serve`, with extra response headers (for rate-limit answers).
pub fn serve_with_headers<F>(handler: F) -> FakeHost
where
    F: Fn(&Recorded) -> (u16, Vec<(String, String)>, String) + Send + Sync + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&requests);
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let Some(request) = read_request(&mut stream) else {
                continue;
            };
            let (status, extra, body) = handler(&request);
            recorded.lock().unwrap().push(request);
            let extra: String = extra
                .iter()
                .map(|(name, value)| format!("{name}: {value}\r\n"))
                .collect();
            let response = format!(
                "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });
    FakeHost {
        base: format!("http://127.0.0.1:{port}"),
        requests,
    }
}

fn read_request(stream: &mut std::net::TcpStream) -> Option<Recorded> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_string();
    let target = parts.next()?.to_string();
    let mut headers = Vec::new();
    let mut length = 0usize;
    loop {
        let mut header = String::new();
        reader.read_line(&mut header).ok()?;
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            let (name, value) = (name.trim().to_string(), value.trim().to_string());
            if name.eq_ignore_ascii_case("content-length") {
                length = value.parse().unwrap_or(0);
            }
            headers.push((name, value));
        }
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).ok()?;
    Some(Recorded {
        method,
        target,
        headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    })
}
