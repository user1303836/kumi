//! The map in the browser (`/map open`): a small server on 127.0.0.1, at a port the system picks, every page under a
//! random 128-bit token, so only who has the link reads them. It answers only a Host of 127.0.0.1 or localhost at its
//! own port (and an Origin, when there is one, of the same), so a page elsewhere can't reach it by another name. It
//! serves:
//! - the page, `/<token>/`: one HTML file with its script inline, fetching nothing from anywhere else;
//! - the points, `/<token>/points`: the map as the page draws it;
//! - what a point plays, `/<token>/audio/<id>`: by the point's id only, never a path, as a WAV of the stretch the
//!   terminal's map plays (so every browser plays it, an AIFF sample too), with Range requests;
//! - one POST, `/<token>/action`: what the producer chose, which becomes a message in the input box;
//! - one POST, `/<token>/palette`: points picked, into the session's palette, answered with what happened in words;
//! - a link that works once, `/once/<nonce>`, which goes on to the page: what the browser is started with, so no
//!   command line (which on Linux every account can read) carries the token. Spent, it finds nothing.

use futures::future::LocalBoxFuture;
use kumi_common::abort::Signal;
use serde_json::Value;
use std::{
    cell::{Cell, RefCell},
    future::Future,
    io,
    path::PathBuf,
    rc::Rc,
    time::Duration,
};
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
    /// Points picked on the page (their ids) into the session's palette: what happened, in words; None when there's no
    /// palette to put them in.
    pub palette: Box<dyn Fn(Vec<String>) -> LocalBoxFuture<'static, Option<String>>>,
}

/// The server, while it runs: closed by `close`, or when it's dropped (Kumi quitting).
pub struct MapServer {
    /// The page's address, with its token: what the producer can paste.
    pub url: String,
    pub port: u16,
    /// The one-time link that goes on to the page: what the browser is started with.
    pub once: String,
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
    let (token, nonce) = (format!("{:032x}", rand::random::<u128>()), format!("{:032x}", rand::random::<u128>()));
    let (url, once) = (format!("http://127.0.0.1:{port}/{token}/"), format!("http://127.0.0.1:{port}/once/{nonce}"));
    let stop = Signal::new();
    let serving = Serving { served, clips: RefCell::default(), once: RefCell::new(Some(nonce)) };
    let (served, stopped) = (Rc::new(serving), stop.clone());
    let open = Rc::new(Cell::new(0usize));
    tokio::task::spawn_local(async move {
        loop {
            tokio::select! {
                _ = stopped.cancelled() => break,
                accepted = listener.accept() => match accepted {
                    // More at once than a page opens: refused.
                    Ok(_) if open.get() >= AT_ONCE => {}
                    Ok((stream, _)) => {
                        let (served, token, stopped) = (served.clone(), token.clone(), stopped.clone());
                        let counted = Counted::new(&open);
                        tokio::task::spawn_local(async move {
                            let _counted = counted;
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
    Ok(MapServer { url, port, once, stop })
}

/// Connections answered at once, at most: a browser opens six to a host.
const AT_ONCE: usize = 16;

/// A connection counted while it's answered.
struct Counted(Rc<Cell<usize>>);

impl Counted {
    fn new(open: &Rc<Cell<usize>>) -> Counted {
        open.set(open.get() + 1);
        Counted(open.clone())
    }
}

impl Drop for Counted {
    fn drop(&mut self) {
        self.0.set(self.0.get() - 1);
    }
}

/// What's served, the clips made last (a browser asks for a clip more than once: its length, then its bytes), and the
/// one-time link's nonce until it's spent.
struct Serving {
    served: Served,
    clips: RefCell<Vec<(String, Rc<Vec<u8>>)>>,
    once: RefCell<Option<String>>,
}

/// Clips kept.
const CLIPS: usize = 4;

/// A request's head at most (a browser sends every cookie it holds for 127.0.0.1, whatever set them), and a POST's
/// body.
const HEAD_MOST: usize = 256 * 1024;
const BODY_MOST: usize = 16 * 1024;
/// How long a request may take to arrive, all of it.
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
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        bytes.extend_from_slice(&chunk[..read]);
    };
    // A header the server doesn't read may hold any bytes (a cookie's).
    let head = String::from_utf8_lossy(&bytes[..end]);
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
        let read = stream.read(&mut chunk).await.ok()?;
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
        302 => "Found",
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
    let Some(request) = tokio::time::timeout(PATIENCE, read_request(&mut stream)).await.ok().flatten() else {
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
    // The one-time link: its first GET goes on to the page, and nothing is there after it.
    if let Some(nonce) = path.strip_prefix("/once/") {
        if request.method != "GET" || serving.once.borrow().as_deref() != Some(nonce) {
            return say(&mut stream, 404, "There's nothing here.").await;
        }
        serving.once.borrow_mut().take();
        let _ = reply(&mut stream, 302, "text/plain", b"", &[("Location", format!("/{token}/"))]).await;
        return;
    }
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
        ("POST", "palette") => {
            let ids: Option<Vec<String>> = serde_json::from_slice::<Value>(&request.body).ok().and_then(|value| {
                value.get("ids")?.as_array()?.iter().map(|id| id.as_str().filter(|id| is_id(id)).map(str::to_string)).collect()
            });
            let said = match ids.filter(|ids| !ids.is_empty() && ids.len() <= kumi_agent::core::palette::MOST_GATHERED) {
                Some(ids) => (served.palette)(ids).await,
                None => None,
            };
            match said {
                Some(said) => {
                    let body = serde_json::json!({ "said": said }).to_string();
                    let _ = reply(&mut stream, 200, "application/json", body.as_bytes(), &[]).await;
                }
                None => say(&mut stream, 400, "That isn't something the map does.").await,
            }
        }
        (_, "" | "points" | "action" | "palette") => say(&mut stream, 405, "Not that way.").await,
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
    let signal = Signal::new();
    let wav = match kept {
        Some(wav) => wav,
        None => match while_asked(stream, &signal, kumi_media::audio::player::stretch(&path, from, seconds, &signal)).await {
            // The page asked for another clip, or went: no one waits for this one.
            None => return,
            Some(Ok((channels, rate))) => {
                let wav = Rc::new(kumi_media::audio::decode::float_wav(&channels, rate.round() as u32));
                let mut clips = serving.clips.borrow_mut();
                clips.retain(|(kept, _)| kept != id);
                clips.insert(0, (id.to_string(), wav.clone()));
                clips.truncate(CLIPS);
                wav
            }
            Some(Err(error)) => return say(stream, 404, &format!("It can't be played: {error}")).await,
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

/// `work`, unless the page that asked goes away first (its connection closes, as when it asks for another clip): then
/// `signal` stops the work, which tidies up after itself, and nothing comes of it. What else the page sends meanwhile
/// is let go.
async fn while_asked<T>(stream: &mut TcpStream, signal: &Signal, work: impl Future<Output = T>) -> Option<T> {
    tokio::pin!(work);
    let mut sent = [0u8; 1024];
    loop {
        tokio::select! {
            done = &mut work => return Some(done),
            read = stream.read(&mut sent) => if !matches!(read, Ok(read) if read > 0) {
                signal.cancel();
                let _ = work.await;
                return None;
            },
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn work_for_a_page_that_went_away_stops_and_a_page_still_there_gets_it() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let client = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let (mut stream, _) = listener.accept().await.unwrap();
        drop(client);
        let signal = Signal::new();
        let work = async {
            signal.cancelled().await;
            "stopped"
        };
        assert_eq!(while_asked(&mut stream, &signal, work).await, None);
        assert!(signal.is_cancelled());
        let _client = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let (mut stream, _) = listener.accept().await.unwrap();
        let signal = Signal::new();
        assert_eq!(while_asked(&mut stream, &signal, async { "made" }).await, Some("made"));
        assert!(!signal.is_cancelled());
        // A page that sends something more meanwhile is still there: the work goes on.
        let mut client = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let (mut stream, _) = listener.accept().await.unwrap();
        tokio::io::AsyncWriteExt::write_all(&mut client, b"more").await.unwrap();
        let signal = Signal::new();
        let work = async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            "made"
        };
        assert_eq!(while_asked(&mut stream, &signal, work).await, Some("made"));
        assert!(!signal.is_cancelled());
    }
}
