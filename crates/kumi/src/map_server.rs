//! The map in the browser (`/map open`): a small server on 127.0.0.1, at a port the system picks, every page under a
//! random 128-bit token, so only who has the link reads them. It answers only a Host of 127.0.0.1 or localhost at its
//! own port (and an Origin, when there is one, of the same), so a page elsewhere can't reach it by another name. It
//! serves:
//! - the page, `/<token>/`: one HTML file with its script inline, fetching nothing from anywhere else;
//! - the points, `/<token>/points`: the map as the page draws it;
//! - what a point plays, `/<token>/audio/<id>`: by the point's id only, never a path, as a WAV of the stretch the
//!   terminal's map plays (so every browser plays it, an AIFF sample too), with Range requests;
//! - one POST, `/<token>/action`: what the producer chose, which becomes a message in the input box.

use futures::future::LocalBoxFuture;
use kumi_common::abort::Signal;
use serde_json::Value;
use std::{cell::RefCell, io, path::PathBuf, rc::Rc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

/// The page: its script and style inline.
pub const PAGE: &str = include_str!("map_page.html");

/// What the map's server serves.
pub struct Served {
    /// The map as the page draws it (JSON), made when the page asks for it.
    pub points: Box<dyn Fn() -> LocalBoxFuture<'static, String>>,
    /// What a point plays: its file, from where and for how long (seconds). None when no point has that id.
    pub audio: Box<dyn Fn(&str) -> Option<(PathBuf, f64, f64)>>,
    /// What the producer chose on the page (an action, and the point's id): true once it's in the input box.
    pub action: Box<dyn Fn(&str, &str) -> bool>,
}

/// The server, while it runs: closed by `close`, or when it's dropped (Kumi quitting).
pub struct MapServer {
    pub url: String,
    pub port: u16,
    stop: Signal,
}

impl MapServer {
    pub fn close(&self) {
        self.stop.cancel();
    }

    pub fn is_closed(&self) -> bool {
        self.stop.is_cancelled()
    }
}

impl Drop for MapServer {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

/// The server started, on Kumi's thread: its page's address, with the token.
pub async fn open(served: Served) -> io::Result<MapServer> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = listener.local_addr()?.port();
    let token = format!("{:032x}", rand::random::<u128>());
    let url = format!("http://127.0.0.1:{port}/{token}/");
    let stop = Signal::new();
    let (served, stopped) = (Rc::new(Serving { served, clips: RefCell::default() }), stop.clone());
    tokio::task::spawn_local(async move {
        loop {
            tokio::select! {
                _ = stopped.cancelled() => break,
                accepted = listener.accept() => match accepted {
                    Ok((stream, _)) => {
                        let (served, token, stopped) = (served.clone(), token.clone(), stopped.clone());
                        tokio::task::spawn_local(async move {
                            tokio::select! {
                                _ = stopped.cancelled() => {}
                                _ = answer(stream, &served, &token, port) => {}
                            }
                        });
                    }
                    Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
                },
            }
        }
    });
    Ok(MapServer { url, port, stop })
}

/// What's served, and the clips made last: a browser asks for a clip more than once (its length, then its bytes).
struct Serving {
    served: Served,
    clips: RefCell<Vec<(String, Rc<Vec<u8>>)>>,
}

/// Clips kept.
const CLIPS: usize = 4;

/// A request's head at most, and a POST's body.
const HEAD_MOST: usize = 16 * 1024;
const BODY_MOST: usize = 16 * 1024;
/// How long a request may take to arrive.
const PATIENCE: Duration = Duration::from_secs(10);

/// A request, as far as the server reads one.
struct Request {
    method: String,
    path: String,
    host: Option<String>,
    origin: Option<String>,
    range: Option<String>,
    body: Vec<u8>,
}

async fn read_request(stream: &mut TcpStream) -> Option<Request> {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 4096];
    let end = loop {
        if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break end;
        }
        if bytes.len() > HEAD_MOST {
            return None;
        }
        let read = tokio::time::timeout(PATIENCE, stream.read(&mut chunk)).await.ok()?.ok()?;
        if read == 0 {
            return None;
        }
        bytes.extend_from_slice(&chunk[..read]);
    };
    let head = std::str::from_utf8(&bytes[..end]).ok()?;
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let (method, path) = (first.next()?.to_string(), first.next()?.to_string());
    let mut request = Request { method, path, host: None, origin: None, range: None, body: vec![] };
    let mut length = 0usize;
    for line in lines {
        let (name, value) = line.split_once(':')?;
        let value = value.trim().to_string();
        match name.trim().to_ascii_lowercase().as_str() {
            "host" => request.host = Some(value),
            "origin" => request.origin = Some(value),
            "range" => request.range = Some(value),
            "content-length" => length = value.parse().ok()?,
            _ => {}
        }
    }
    if length > BODY_MOST {
        return None;
    }
    request.body = bytes[end + 4..].to_vec();
    while request.body.len() < length {
        let read = tokio::time::timeout(PATIENCE, stream.read(&mut chunk)).await.ok()?.ok()?;
        if read == 0 {
            return None;
        }
        request.body.extend_from_slice(&chunk[..read]);
    }
    request.body.truncate(length);
    Some(request)
}

/// What every answer says beyond its own: nothing kept, nothing guessed, nothing told where it came from.
const ALWAYS: &str = "Connection: close\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\n";
/// The page fetches only from its own server, and runs only its own script.
const PAGE_POLICY: &str = "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; connect-src 'self'; media-src 'self'; img-src 'self' data:; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

async fn reply(stream: &mut TcpStream, status: u16, kind: &str, body: &[u8], extra: &[(&str, String)]) -> io::Result<()> {
    let reason = match status {
        200 => "OK",
        204 => "No Content",
        206 => "Partial Content",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        416 => "Range Not Satisfiable",
        _ => "Internal Server Error",
    };
    let mut head = format!("HTTP/1.1 {status} {reason}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\n{ALWAYS}", body.len());
    for (name, value) in extra {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(body).await?;
    stream.flush().await
}

async fn say(stream: &mut TcpStream, status: u16, words: &str) {
    let _ = reply(stream, status, "text/plain; charset=utf-8", words.as_bytes(), &[]).await;
}

/// A point's id: letters, digits, `:`, `-` and `_` (`sounds:12`, `songs:345`), never a path.
pub fn is_id(text: &str) -> bool {
    !text.is_empty() && text.len() <= 64 && text.bytes().all(|byte| byte.is_ascii_alphanumeric() || b":-_".contains(&byte))
}

async fn answer(mut stream: TcpStream, serving: &Serving, token: &str, port: u16) {
    let served = &serving.served;
    let Some(request) = read_request(&mut stream).await else {
        return say(&mut stream, 400, "That isn't a request this page makes.").await;
    };
    let ours = [format!("127.0.0.1:{port}"), format!("localhost:{port}")];
    let host = request.host.as_deref().is_some_and(|host| ours.iter().any(|ours| ours.eq_ignore_ascii_case(host)));
    let origin =
        request.origin.as_deref().is_none_or(|origin| ours.iter().any(|ours| origin.eq_ignore_ascii_case(&format!("http://{ours}"))));
    if !host || !origin {
        return say(&mut stream, 403, "This map answers only its own page.").await;
    }
    let path = request.path.split('?').next().unwrap_or_default();
    let Some(asked) = path.strip_prefix(&format!("/{token}/")) else {
        return say(&mut stream, 404, "There's nothing here.").await;
    };
    match (request.method.as_str(), asked) {
        ("GET", "") => {
            let _ =
                reply(&mut stream, 200, "text/html; charset=utf-8", PAGE.as_bytes(), &[("Content-Security-Policy", PAGE_POLICY.into())])
                    .await;
        }
        ("GET", "points") => {
            let points = (served.points)().await;
            let _ = reply(&mut stream, 200, "application/json", points.as_bytes(), &[]).await;
        }
        ("GET", asked) if asked.starts_with("audio/") => {
            audio(&mut stream, serving, &asked["audio/".len()..], request.range.as_deref()).await
        }
        ("POST", "action") => {
            let chosen: Option<(String, String)> = serde_json::from_slice::<Value>(&request.body)
                .ok()
                .and_then(|value| Some((value.get("action")?.as_str()?.to_string(), value.get("id")?.as_str()?.to_string())));
            match chosen {
                Some((action, id)) if is_id(&id) && (served.action)(&action, &id) => {
                    let _ = reply(&mut stream, 204, "text/plain", b"", &[]).await;
                }
                _ => say(&mut stream, 400, "That isn't something the map does.").await,
            }
        }
        (_, "" | "points" | "action") => say(&mut stream, 405, "Not that way.").await,
        _ => say(&mut stream, 404, "There's nothing here.").await,
    }
}

/// A path's segment with its %-escapes read, as a browser sends an id (`sounds%3A12` is `sounds:12`); none when an
/// escape isn't two hex digits or what they make isn't UTF-8.
pub fn unescaped(segment: &str) -> Option<String> {
    let (bytes, mut at) = (segment.as_bytes(), 0);
    let mut out = Vec::with_capacity(bytes.len());
    while at < bytes.len() {
        if bytes[at] == b'%' {
            let hex = segment.get(at + 1..at + 3).filter(|hex| hex.bytes().all(|byte| byte.is_ascii_hexdigit()))?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            at += 3;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// What a point plays, as a WAV of its stretch: all of it, or the bytes a Range asks for.
async fn audio(stream: &mut TcpStream, serving: &Serving, id: &str, range: Option<&str>) {
    let Some(id) = unescaped(id).filter(|id| is_id(id)) else {
        return say(stream, 400, "Ask for a point by its id.").await;
    };
    let id = id.as_str();
    let Some((path, from, seconds)) = (serving.served.audio)(id) else {
        return say(stream, 404, "No point has that id.").await;
    };
    let kept = serving.clips.borrow().iter().find(|(kept, _)| kept == id).map(|(_, wav)| wav.clone());
    let wav = match kept {
        Some(wav) => wav,
        None => match kumi_media::audio::player::stretch(&path, from, seconds, &Signal::new()).await {
            Ok((channels, rate)) => {
                let wav = Rc::new(kumi_media::audio::decode::float_wav(&channels, rate.round() as u32));
                let mut clips = serving.clips.borrow_mut();
                clips.retain(|(kept, _)| kept != id);
                clips.insert(0, (id.to_string(), wav.clone()));
                clips.truncate(CLIPS);
                wav
            }
            Err(error) => return say(stream, 404, &format!("It can't be played: {error}")).await,
        },
    };
    let size = wav.len() as u64;
    let ranges = [("Accept-Ranges", "bytes".to_string())];
    match range {
        None => {
            let _ = reply(stream, 200, "audio/wav", &wav, &ranges).await;
        }
        Some(range) => match byte_range(range, size) {
            Some((first, last)) => {
                let part = &wav[first as usize..=last as usize];
                let extra = [ranges[0].clone(), ("Content-Range", format!("bytes {first}-{last}/{size}"))];
                let _ = reply(stream, 206, "audio/wav", part, &extra).await;
            }
            None => {
                let _ = reply(stream, 416, "text/plain", b"", &[("Content-Range", format!("bytes */{size}"))]).await;
            }
        },
    }
}

/// A single byte range (`bytes=first-last`, `first-` or `-last count`) of `size` bytes, its ends within them.
pub fn byte_range(header: &str, size: u64) -> Option<(u64, u64)> {
    let (first, last) = header.trim().strip_prefix("bytes=")?.split_once('-')?;
    if size == 0 || first.contains(',') || last.contains(',') {
        return None;
    }
    let (first, last) = match (first.trim(), last.trim()) {
        ("", count) => {
            let count: u64 = count.parse().ok()?;
            (size.saturating_sub(count), size - 1)
        }
        (first, "") => (first.parse().ok()?, size - 1),
        (first, last) => (first.parse().ok()?, last.parse::<u64>().ok()?.min(size - 1)),
    };
    (first <= last && first < size).then_some((first, last))
}
