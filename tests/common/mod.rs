//! A server on this machine that records what it is sent, for the tests of
//! `--report-to`, `push` and `--openlineage`.
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;

use serde_json::Value;

/// One request as the server read it.
pub struct Received {
    pub request_line: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Received {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap()
    }
}

/// A server on a loopback port that answers every request with `status` and
/// hands what it read to the returned channel. A 3xx names a `Location`.
pub fn server(status: u16) -> (String, mpsc::Receiver<Received>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1/runs", listener.local_addr().unwrap());
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            let mut headers = Vec::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let line = line.trim_end();
                if line.is_empty() {
                    break;
                }
                let (k, v) = line.split_once(':').unwrap();
                headers.push((k.trim().to_string(), v.trim().to_string()));
            }
            let length: usize = headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                .map(|(_, v)| v.parse().unwrap())
                .unwrap_or(0);
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            // A redirect points off this machine, over plain http.
            let location = match status {
                300..=399 => "Location: http://ingest.example.com/v1/runs\r\n",
                _ => "",
            };
            write!(
                stream,
                "HTTP/1.1 {status} Status\r\n{location}Content-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
            let _ = tx.send(Received {
                request_line: request_line.trim_end().to_string(),
                headers,
                body,
            });
        }
    });
    (url, rx)
}
