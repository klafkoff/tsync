//! Session tests against a loopback fake of qBittorrent's `WebAPI`.
//!
//! This is a local socket, not the network: no tracker, no SSH, no registry.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use tsync_qbt::{Client, Session};

#[derive(Default)]
struct State {
    requests: Vec<String>,
}

#[derive(Clone, Copy)]
enum Dialect {
    Legacy,
    /// `WebAPI` 2.9 — `pause` / `resume`, not `stop` / `start`.
    V29,
    V52,
}

fn spawn() -> (String, Arc<Mutex<State>>, thread::JoinHandle<()>) {
    spawn_dialect(Dialect::Legacy)
}

fn spawn_v52() -> (String, Arc<Mutex<State>>, thread::JoinHandle<()>) {
    spawn_dialect(Dialect::V52)
}

fn spawn_dialect(dialect: Dialect) -> (String, Arc<Mutex<State>>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    listener.set_nonblocking(true).expect("nonblocking");
    let state = Arc::new(Mutex::new(State::default()));
    let shared = Arc::clone(&state);
    let handle = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(4);
        while Instant::now() < deadline {
            match listener.accept() {
                Ok((stream, _)) => serve(stream, &shared, dialect),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5));
                }
                Err(_) => break,
            }
        }
    });
    (format!("http://{addr}"), state, handle)
}

fn serve(mut stream: TcpStream, state: &Arc<Mutex<State>>, dialect: Dialect) {
    stream
        .set_read_timeout(Some(Duration::from_millis(250)))
        .ok();
    let Ok((method, path, body)) = read_request(&mut stream) else {
        return;
    };
    state
        .lock()
        .expect("state")
        .requests
        .push(format!("{method} {path} {body}"));

    let (status, headers, reply) = match (method.as_str(), path.as_str()) {
        ("POST", "/api/v2/auth/login") if body.contains("password=secret") => match dialect {
            Dialect::Legacy | Dialect::V29 => (
                "200 OK",
                "Set-Cookie: SID=test-sid; HttpOnly; Path=/\r\n",
                b"Ok.".as_slice(),
            ),
            Dialect::V52 => (
                "204 No Content",
                "Set-Cookie: QBT_SID_8080=test-sid; HttpOnly; SameSite=Strict\r\n",
                b"".as_slice(),
            ),
        },
        ("POST", "/api/v2/auth/login") => match dialect {
            Dialect::Legacy | Dialect::V29 => ("200 OK", "", b"Fails.".as_slice()),
            Dialect::V52 => ("401 Unauthorized", "", b"Unauthorized".as_slice()),
        },
        ("GET", "/api/v2/app/webapiVersion") => match dialect {
            Dialect::V29 => ("200 OK", "", b"2.9.3".as_slice()),
            Dialect::Legacy | Dialect::V52 => ("200 OK", "", b"2.11.2".as_slice()),
        },
        ("GET", "/api/v2/torrents/info") => (
            "200 OK",
            "",
            br#"[{"hash":"aa","name":"red","state":"stalledUP","progress":1.0,"amount_left":0}]"#
                .as_slice(),
        ),
        ("GET", "/api/v2/transfer/downloadLimit") => ("200 OK", "", b"0".as_slice()),
        ("GET", path) if path.starts_with("/api/v2/torrents/files") => (
            "200 OK",
            "",
            br#"[{"name":"red/a.flac","size":1024,"progress":1.0}]"#.as_slice(),
        ),
        ("POST", "/api/v2/torrents/add") => match dialect {
            Dialect::Legacy | Dialect::V29 => ("200 OK", "", b"Ok.".as_slice()),
            Dialect::V52 => (
                "200 OK",
                "",
                br#"{"added_torrent_ids":["aa"],"failure_count":0,"pending_count":0,"success_count":1}"#
                    .as_slice(),
            ),
        },
        (
            "POST",
            "/api/v2/torrents/stop"
            | "/api/v2/torrents/start"
            | "/api/v2/torrents/pause"
            | "/api/v2/torrents/resume"
            | "/api/v2/torrents/recheck"
            | "/api/v2/transfer/setDownloadLimit",
        ) => ("200 OK", "", b"".as_slice()),
        _ => ("404 Not Found", "", b"".as_slice()),
    };

    let header = format!(
        "HTTP/1.1 {status}\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         {headers}\
         \r\n",
        reply.len()
    );
    if stream.write_all(header.as_bytes()).is_err() {
        return;
    }
    let _ = stream.write_all(reply);
}

fn read_request(stream: &mut TcpStream) -> std::io::Result<(String, String, String)> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 1024];
    loop {
        let n = stream.read(&mut tmp)?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(header_end) = find_header_end(&buf) {
            let headers = std::str::from_utf8(&buf[..header_end])
                .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
            let mut lines = headers.split("\r\n");
            let request = lines.next().unwrap_or("");
            let mut parts = request.split_whitespace();
            let method = parts.next().unwrap_or("").to_owned();
            let path = parts.next().unwrap_or("").to_owned();
            let content_length = headers
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|value| value.trim().parse::<usize>().unwrap_or(0))
                })
                .unwrap_or(0);
            while buf.len() < header_end + content_length {
                let n = stream.read(&mut tmp)?;
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&tmp[..n]);
            }
            let body =
                String::from_utf8_lossy(&buf[header_end..header_end + content_length]).into_owned();
            return Ok((method, path, body));
        }
        if n < tmp.len() && buf.len() > 16 && !buf.windows(4).any(|w| w == b"\r\n\r\n") {
            // incomplete headers; keep reading
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::UnexpectedEof,
        "no request",
    ))
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|i| i + 4)
}

#[test]
fn login_rejects_bad_password() {
    let (url, _, _keep) = spawn();
    let error = Session::login(&url, "admin", "wrong").expect_err("denied");
    assert!(error.to_string().contains("login"));
}

#[test]
fn login_rejects_qbt_5_2_401() {
    let (url, _, _keep) = spawn_v52();
    let error = Session::login(&url, "admin", "wrong").expect_err("denied");
    assert!(error.to_string().contains("login"));
}

#[test]
fn login_accepts_qbt_5_2_204_and_port_cookie() {
    let (url, state, _keep) = spawn_v52();
    let session = Session::login(&url, "admin", "secret").expect("login");
    session
        .add_paused(b"dummy", "aa.torrent", "/data")
        .expect("add");
    let log = state.lock().expect("log").requests.join("\n");
    assert!(log.contains("POST /api/v2/torrents/add"));
}

#[test]
fn login_lists_and_adds_paused() {
    let (url, state, _keep) = spawn();
    let session = Session::login(&url, "admin", "secret").expect("login");
    let torrents = session.list().expect("list");
    assert_eq!(torrents.len(), 1);
    assert_eq!(torrents[0].state, "stalledUP");

    session
        .add_paused(b"dummy", "aa.torrent", "/data")
        .expect("add");
    session.stop("aa").expect("stop");
    session.start("aa").expect("start");
    session.recheck("aa").expect("recheck");
    assert_eq!(session.download_limit().expect("limit"), 0);
    session.set_download_limit(1).expect("set limit");
    let files = session.files("aa").expect("files");
    assert_eq!(files.len(), 1);
    assert!(files[0].progress >= 1.0);

    let log = state.lock().expect("log").requests.join("\n");
    assert!(log.contains("POST /api/v2/torrents/add"));
    assert!(log.contains("name=\"paused\""));
    assert!(log.contains("name=\"stopped\""));
    assert!(log.contains("FilesChecked"));
    assert!(log.contains("POST /api/v2/torrents/stop"));
    assert!(log.contains("POST /api/v2/torrents/start"));
    assert!(!log.contains("POST /api/v2/torrents/pause"));
    assert!(!log.contains("POST /api/v2/torrents/resume"));
}

fn spawn_v29() -> (String, Arc<Mutex<State>>, thread::JoinHandle<()>) {
    spawn_dialect(Dialect::V29)
}

#[test]
fn login_uses_pause_and_resume_on_webapi_2_9() {
    let (url, state, _keep) = spawn_v29();
    let session = Session::login(&url, "admin", "secret").expect("login");
    session.stop("aa").expect("pause");
    session.start("aa").expect("resume");

    let log = state.lock().expect("log").requests.join("\n");
    assert!(log.contains("POST /api/v2/torrents/pause"));
    assert!(log.contains("POST /api/v2/torrents/resume"));
    assert!(!log.contains("POST /api/v2/torrents/stop"));
    assert!(!log.contains("POST /api/v2/torrents/start"));
}
