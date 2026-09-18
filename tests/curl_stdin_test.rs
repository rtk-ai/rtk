#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::{Command, Stdio};

/// Accept one HTTP request on `listener` and return the exact request body.
fn read_one_request_body(listener: TcpListener) -> Vec<u8> {
    let (mut stream, _) = listener.accept().expect("accept connection");

    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    // Read until end of headers.
    let header_end = loop {
        let n = stream.read(&mut chunk).expect("read request");
        assert!(n > 0, "connection closed before headers were complete");
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
    };

    let headers = String::from_utf8_lossy(&buf[..header_end]).into_owned();
    let content_length: usize = headers
        .lines()
        .find_map(|l| {
            let (name, value) = l.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().ok())?
        })
        .expect("Content-Length header");

    let mut body = buf[header_end..].to_vec();
    while body.len() < content_length {
        let n = stream.read(&mut chunk).expect("read body");
        assert!(n > 0, "connection closed before body was complete");
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(content_length);

    // Reply so curl exits 0 instead of erroring with "empty reply".
    stream
        .write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\nContent-Length: 0\r\n\r\n")
        .expect("write response");
    body
}

#[test]
fn curl_forwards_piped_stdin_body() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("local addr").port();
    let server = std::thread::spawn(move || read_one_request_body(listener));

    let body = b"{\"via\":\"heredoc\"}";
    let mut child = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args([
            "curl",
            "-sS",
            "-X",
            "PUT",
            "-H",
            "Content-Type: application/json",
            "-d",
            "@-",
            &format!("http://127.0.0.1:{port}/t"),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn rtk curl");
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(body)
        .expect("write stdin");

    let output = child.wait_with_output().expect("wait for rtk curl");
    assert!(
        output.status.success(),
        "rtk curl failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let received = server.join().expect("server thread");
    assert_eq!(received, body, "server must receive the exact stdin body");
}
