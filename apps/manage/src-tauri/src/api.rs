//! Relay API client. Every admin request is signed with `tether_trust::reqauth` over the exact
//! path (with query) and body bytes that go on the wire.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tether_trust::{reqauth, SigningKey};

pub fn shared() -> &'static reqwest::Client {
    client()
}

fn client() -> &'static reqwest::Client {
    static C: OnceLock<reqwest::Client> = OnceLock::new();
    C.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            // Release uploads may be up to 200 MiB on a slow uplink; STALL and ANSWER end a
            // transfer to a relay that stops taking part long before this.
            .timeout(FILE)
            // A signed request (or an API token, cdn.rs) is never sent on to wherever a relay points.
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("tether-manage/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("http client")
    })
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

fn explain(status: u16, body: &str) -> String {
    let reason = body.trim();
    let reason = if reason.is_empty() { "no details".to_owned() } else { reason.chars().take(300).collect() };
    let hint = match status {
        300..=399 => "The server redirects to another address, which this app does not follow. Enter the server's own https:// address.",
        // Requests are signed for one relay host (v2); the three ways a relay is not ready for that.
        401 if reason.contains("unsupported signature version") => "This relay runs an older tether-api that does not know this app's request signatures. Update the relay first.",
        401 if reason.contains("RELAY_HOST") => "The relay does not know its own host name. Set RELAY_HOST on the relay to the name entered under Status and relays.",
        401 if reason.contains("signed for another relay") => "The relay knows itself under another host name. Enter the relay's own name (its RELAY_HOST) under Status and relays.",
        401 => "The server did not accept this app's admin signature. Check that the server's ADMIN_PUB_KEY contains this app's admin public key and that the PC clock is correct.",
        400 if reason.contains("unknown feature") => "This relay runs an older tether-api that does not know a feature of this policy. Update the relay first, then publish again.",
        404 => "Not found on the server.",
        409 if reason.starts_with("published release") => "The relay keeps the published release.",
        409 => "The server already has a newer version. Reload and try again.",
        413 => "The upload is larger than the server allows.",
        429 => "Too many requests. Wait a minute and try again.",
        507 => "The server is low on disk space (less than 2 GiB would remain).",
        500..=599 => "The server failed to handle the request.",
        _ => "The server refused the request.",
    };
    format!("{hint} (HTTP {status}: {reason})")
}

fn url(server: &str, path: &str) -> String {
    format!("{}{path}", server.trim_end_matches('/'))
}

/// Small reads a view waits for (live policy, relay health): no answer within this counts as
/// unreachable, instead of the client's 30 min upload timeout. Also the limit for the first byte
/// of a file download ([`admin_download`], [`public_download`]).
pub const QUICK: Duration = Duration::from_secs(10);
/// Small signed writes (JSON documents, the download page; at most 512 KiB): the relay answers
/// at once, so it must start answering within [`QUICK`] and finish within this. Only release file
/// uploads ([`admin_upload`]) and copies ([`file`]) get [`FILE`].
pub const WRITE: Duration = Duration::from_secs(30);
/// A diagnostics upload (at most 2 MiB) from start to end, once it has started within [`QUICK`].
pub const DIAG: Duration = Duration::from_secs(60);
/// The macOS disk image (about 17 MB) the download page's facts are taken from.
pub const DMG: Duration = Duration::from_secs(180);
/// A release file (upload or copy) from start to end, for slow links. [`STALL`] and [`ANSWER`]
/// end a transfer to a relay that stops taking part long before this.
pub const FILE: Duration = Duration::from_secs(30 * 60);
/// While a file goes out or comes in: no progress for this long fails that relay.
pub const STALL: Duration = if cfg!(test) { Duration::from_secs(2) } else { Duration::from_secs(30) };
/// After the last byte of an upload the relay checks the signature over the whole file and moves
/// it into place before it answers (200 MB on a local relay: see docs/management.md).
pub const ANSWER: Duration = if cfg!(test) { Duration::from_secs(2) } else { Duration::from_secs(30) };
pub const CANCELLED: &str = "Cancelled.";

/// The release publish or copy in progress (its run id) and the run the last Cancel was for.
static RUN: AtomicU64 = AtomicU64::new(0);
static CANCEL: AtomicU64 = AtomicU64::new(u64::MAX);
/// Run ids for calls without one (the UI's ids are below 2^63).
static OWN: AtomicU64 = AtomicU64::new(1 << 63);

/// A release publish or copy starts or goes on with run `run`: the UI's id for one press of
/// Publish, which may publish several builds one after another. A Cancel of that run that came
/// before (between two builds) stays in force. `None`: a run of its own.
pub fn begin(run: Option<u64>) {
    RUN.store(run.unwrap_or_else(|| OWN.fetch_add(1, Ordering::SeqCst)), Ordering::SeqCst);
}

/// Cancel run `run` (`None`: the one in progress): every upload and file copy of it ends with
/// [`CANCELLED`], and the publish stops before its first release index write.
pub fn cancel(run: Option<u64>) {
    CANCEL.store(run.unwrap_or_else(|| RUN.load(Ordering::SeqCst)), Ordering::SeqCst);
}

pub fn cancelled() -> bool {
    CANCEL.load(Ordering::SeqCst) == RUN.load(Ordering::SeqCst)
}

/// `f`, or [`CANCELLED`] as soon as the run is cancelled ([`cancel`]).
pub async fn cancellable<T>(f: impl std::future::Future<Output = Result<T, String>>) -> Result<T, String> {
    let watch = async {
        while !cancelled() {
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    };
    match futures_util::future::select(std::pin::pin!(f), std::pin::pin!(watch)).await {
        futures_util::future::Either::Left((r, _)) => r,
        futures_util::future::Either::Right(_) => Err(CANCELLED.to_owned()),
    }
}

/// Signed admin request with a small body: the answer must start within [`QUICK`] and be complete
/// within [`WRITE`]. Returns the body on 2xx.
pub async fn admin(server: &str, key: &SigningKey, method: &str, path: &str, body: Vec<u8>, content_type: &str) -> Result<Vec<u8>, String> {
    let m = reqwest::Method::from_bytes(method.as_bytes()).map_err(|e| e.to_string())?;
    let sig = reqauth::sign_request_for(key, server, method, path, &body, now());
    let mut req = client().request(m, url(server, path)).header(reqauth::HEADER, sig).timeout(WRITE);
    if !body.is_empty() {
        req = req.header("Content-Type", content_type).body(body);
    }
    read_body(started(req).await?).await
}

/// Signed `GET` of a file: the answer must start within [`QUICK`] and be complete within `total`.
pub async fn admin_download(server: &str, key: &SigningKey, path: &str, total: Duration) -> Result<Vec<u8>, String> {
    let sig = reqauth::sign_request_for(key, server, "GET", path, b"", now());
    read_body(started(client().get(url(server, path)).header(reqauth::HEADER, sig).timeout(total)).await?).await
}

/// Unauthenticated `GET` of a file with the limits of [`admin_download`]; `Ok(None)` on 404.
pub async fn public_download(server: &str, path: &str, total: Duration) -> Result<Option<Vec<u8>>, String> {
    match read_body(started(client().get(url(server, path)).timeout(total)).await?).await {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.contains("HTTP 404") => Ok(None),
        Err(e) => Err(e),
    }
}

/// Sends `req`; a relay that accepts the connection but sends no answer within [`QUICK`] fails.
async fn started(req: reqwest::RequestBuilder) -> Result<reqwest::Response, String> {
    match tokio::time::timeout(QUICK, req.send()).await {
        Ok(r) => r.map_err(unreachable),
        Err(_) => Err(NO_ANSWER.to_owned()),
    }
}

/// Signed `GET` that gives up after [`QUICK`].
pub async fn admin_quick(server: &str, key: &SigningKey, path: &str) -> Result<Vec<u8>, String> {
    let sig = reqauth::sign_request_for(key, server, "GET", path, b"", now());
    finish(client().get(url(server, path)).header(reqauth::HEADER, sig).timeout(QUICK).send().await).await
}

/// [`admin_quick`], and whether the answer carries the response header `flag`.
pub async fn admin_quick_flag(server: &str, key: &SigningKey, path: &str, flag: &str) -> Result<(Vec<u8>, bool), String> {
    let sig = reqauth::sign_request_for(key, server, "GET", path, b"", now());
    let res = client().get(url(server, path)).header(reqauth::HEADER, sig).timeout(QUICK).send().await.map_err(unreachable)?;
    let set = res.headers().contains_key(flag);
    Ok((read_body(res).await?, set))
}

/// Signed `PUT` of a release file with the limits of [`upload`]; `sent(bytes so far)` runs as the
/// body goes out (256 KiB steps).
pub async fn admin_upload(server: &str, key: &SigningKey, path: &str, body: Vec<u8>, content_type: &str, sent: impl Fn(u64) + Send + Sync + 'static) -> Result<Vec<u8>, String> {
    let sig = reqauth::sign_request_for(key, server, "PUT", path, &body, now());
    let req = client().put(url(server, path)).header(reqauth::HEADER, sig).header("Content-Type", content_type);
    let res = upload(req, body, sent).await?;
    cancellable(read_body(res)).await
}

/// Sends `req` with `body` streamed in 256 KiB chunks, `sent(bytes so far)` as each one goes out.
/// Fails when no chunk goes out for [`STALL`], when the answer has not started [`ANSWER`] after
/// the last one, or on [`cancel`]. Returns the response once its head has arrived.
pub async fn upload(req: reqwest::RequestBuilder, body: Vec<u8>, sent: impl Fn(u64) + Send + Sync + 'static) -> Result<reqwest::Response, String> {
    let len = body.len() as u64;
    let chunks: Vec<Vec<u8>> = body.chunks(256 * 1024).map(<[u8]>::to_vec).collect();
    // When the last chunk went out, and the bytes so far.
    let mark = Arc::new(Mutex::new((Instant::now(), 0u64)));
    let (m, mut done) = (mark.clone(), 0u64);
    // Lazy: a chunk is counted when the connection pulls it.
    let stream = futures_util::stream::iter(chunks.into_iter().map(move |c| {
        done += c.len() as u64;
        *m.lock().unwrap() = (Instant::now(), done);
        sent(done);
        Ok::<_, std::io::Error>(c)
    }));
    // Explicit length: a streamed body would otherwise go out chunked.
    let send = async { req.header("Content-Length", len).body(reqwest::Body::wrap_stream(stream)).send().await.map_err(unreachable) };
    let watch = async {
        loop {
            tokio::time::sleep(Duration::from_millis(250)).await;
            let (at, n) = *mark.lock().unwrap();
            if n < len && at.elapsed() > STALL {
                return format!("The upload stalled: the server took nothing for {} s ({n} of {len} bytes sent).", STALL.as_secs());
            }
            if n == len && at.elapsed() > ANSWER {
                return format!("No answer from the server within {} s after the upload.", ANSWER.as_secs());
            }
        }
    };
    let both = async {
        match futures_util::future::select(std::pin::pin!(send), std::pin::pin!(watch)).await {
            futures_util::future::Either::Left((r, _)) => r,
            futures_util::future::Either::Right((e, _)) => Err(e),
        }
    };
    cancellable(both).await
}

/// Unauthenticated `HEAD` of a public file: `Ok(false)` on 404. Gives up after [`QUICK`].
pub async fn exists(server: &str, path: &str) -> Result<bool, String> {
    match finish(client().head(url(server, path)).timeout(QUICK).send().await).await {
        Ok(_) => Ok(true),
        Err(e) if e.contains("HTTP 404") => Ok(false),
        Err(e) => Err(e),
    }
}

/// A release file from a relay (release copy, CDN): [`public_download`] within [`FILE`], ended
/// by [`cancel`]. `Ok(None)` on 404.
pub async fn file(server: &str, path: &str) -> Result<Option<Vec<u8>>, String> {
    cancellable(public_download(server, path, FILE)).await
}

/// [`public`] that gives up after [`QUICK`].
pub async fn public_quick(server: &str, path: &str) -> Result<Option<Vec<u8>>, String> {
    public_req(client().get(url(server, path)).timeout(QUICK)).await
}

async fn public_req(req: reqwest::RequestBuilder) -> Result<Option<Vec<u8>>, String> {
    match finish(req.send().await).await {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.contains("HTTP 404") => Ok(None),
        Err(e) => Err(e),
    }
}

const NO_ANSWER: &str = "No answer from the server in time.";

/// `e` (from this module) says the relay did not answer or broke off, not what it answered.
pub fn unreachable_error(e: &str) -> bool {
    ["No answer from the server", "Could not reach the server", "stopped sending", "stopped answering", "connection broke", "upload stalled"].iter().any(|p| e.contains(p))
}

fn unreachable(e: reqwest::Error) -> String {
    if e.is_timeout() { NO_ANSWER.to_owned() } else { format!("Could not reach the server: {e}") }
}

async fn finish(res: reqwest::Result<reqwest::Response>) -> Result<Vec<u8>, String> {
    read_body(res.map_err(unreachable)?).await
}

async fn read_body(res: reqwest::Response) -> Result<Vec<u8>, String> {
    let (status, bytes) = read_all(res).await?;
    if (200..300).contains(&status) {
        Ok(bytes)
    } else {
        Err(explain(status, &String::from_utf8_lossy(&bytes)))
    }
}

/// Status and whole body, whatever the status; a server that sends nothing for [`STALL`] fails.
pub async fn read_all(mut res: reqwest::Response) -> Result<(u16, Vec<u8>), String> {
    let status = res.status().as_u16();
    let mut bytes = Vec::new();
    loop {
        match tokio::time::timeout(STALL, res.chunk()).await {
            Err(_) => return Err(format!("The server stopped sending (nothing for {} s).", STALL.as_secs())),
            Ok(Err(e)) => return Err(if e.is_timeout() { "The server stopped answering in time.".to_owned() } else { format!("The connection broke while reading the answer: {e}") }),
            Ok(Ok(None)) => break,
            Ok(Ok(Some(c))) => bytes.extend_from_slice(&c),
        }
    }
    Ok((status, bytes))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;

    /// A one-shot HTTP server that checks the `Tether-Sig` header the way tether-api does.
    fn one_shot(pk: tether_trust::VerifyingKey) -> (String, std::thread::JoinHandle<(String, String, Vec<u8>)>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let server = format!("http://{}", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let (s, _) = listener.accept().unwrap();
            let mut r = BufReader::new(s.try_clone().unwrap());
            let mut line = String::new();
            r.read_line(&mut line).unwrap();
            let mut parts = line.split_whitespace();
            let (method, path) = (parts.next().unwrap().to_owned(), parts.next().unwrap().to_owned());
            let (mut sig, mut len) = (String::new(), 0usize);
            loop {
                let mut h = String::new();
                r.read_line(&mut h).unwrap();
                if h.trim().is_empty() {
                    break;
                }
                let (k, v) = h.split_once(':').unwrap();
                match k.to_ascii_lowercase().as_str() {
                    "tether-sig" => sig = v.trim().to_owned(),
                    "content-length" => len = v.trim().parse().unwrap(),
                    _ => {}
                }
            }
            let mut body = vec![0; len];
            r.read_exact(&mut body).unwrap();
            let relay = reqauth::Relay { hosts: &["127.0.0.1".to_owned()], v1: false };
            let ok = reqauth::verify_request_for(&sig, &pk, relay, &method, &path, &reqauth::body_digest(&body), now(), |_| true);
            let (code, text) = if ok.is_ok() { (200, "{\"ok\":true}".to_owned()) } else { (401, ok.unwrap_err()) };
            write!(&s, "HTTP/1.1 {code} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}", text.len()).unwrap();
            (method, path, body)
        });
        (server, handle)
    }

    #[test]
    fn signed_request_is_accepted_by_verify_request() {
        let key = tether_trust::generate_key();
        let (server, handle) = one_shot(key.verifying_key());
        let out = tauri::async_runtime::block_on(admin(&server, &key, "POST", "/api/admin/diag-ticket?x=1", br#"{"note":"hi"}"#.to_vec(), "application/json"));
        assert_eq!(out.unwrap(), b"{\"ok\":true}");
        let (m, p, b) = handle.join().unwrap();
        assert_eq!((m.as_str(), p.as_str(), b.as_slice()), ("POST", "/api/admin/diag-ticket?x=1", &br#"{"note":"hi"}"#[..]));
    }

    /// Streamed with a Content-Length (the test server reads exactly that), signed over the whole
    /// body, and the progress ends at the full size.
    #[test]
    fn upload_streams_with_progress() {
        let _one = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let key = tether_trust::generate_key();
        let (server, handle) = one_shot(key.verifying_key());
        let data: Vec<u8> = (0..600_000u32).map(|i| i as u8).collect();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let s2 = seen.clone();
        let out = tauri::async_runtime::block_on(admin_upload(&server, &key, "/api/admin/release/stable/x/a.exe", data.clone(), "application/octet-stream", move |n| s2.lock().unwrap().push(n)));
        assert_eq!(out.unwrap(), b"{\"ok\":true}");
        let (m, _, b) = handle.join().unwrap();
        assert_eq!((m.as_str(), b == data), ("PUT", true));
        assert_eq!(*seen.lock().unwrap(), [262_144, 524_288, 600_000]);
    }

    /// A relay that accepts the connection and never answers: a download and a small write give
    /// up after QUICK, not after their total limit.
    #[test]
    fn silent_relay_fails_quickly() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let server = format!("http://{}", listener.local_addr().unwrap());
        let hold = std::thread::spawn(move || {
            let (s, _) = listener.accept().unwrap();
            std::thread::sleep(QUICK + Duration::from_secs(3));
            drop(s);
        });
        let key = tether_trust::generate_key();
        let t = std::time::Instant::now();
        let r = tauri::async_runtime::block_on(admin_download(&server, &key, "/api/admin/diag/AAAAAAAA", Duration::from_secs(600)));
        assert_eq!(r.unwrap_err(), NO_ANSWER);
        assert!(t.elapsed() < QUICK + Duration::from_secs(2), "{:?}", t.elapsed());
        hold.join().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let server = format!("http://{}", listener.local_addr().unwrap());
        let hold = std::thread::spawn(move || {
            let (s, _) = listener.accept().unwrap();
            std::thread::sleep(QUICK + Duration::from_secs(3));
            drop(s);
        });
        let t = std::time::Instant::now();
        let r = tauri::async_runtime::block_on(admin(&server, &key, "POST", "/api/admin/diag-ticket", b"{}".to_vec(), "application/json"));
        assert_eq!(r.unwrap_err(), NO_ANSWER);
        assert!(t.elapsed() < QUICK + Duration::from_secs(2), "{:?}", t.elapsed());
        hold.join().unwrap();
    }

    /// Uploads that can be cancelled (the cancel switch is process-wide) run one at a time.
    pub(crate) static SERIAL: Mutex<()> = Mutex::new(());

    /// Takes one connection and never answers: reads the whole body (`read`) or nothing, for `hold`.
    fn silent(read: bool, hold: Duration) -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let server = format!("http://{}", listener.local_addr().unwrap());
        let h = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let end = Instant::now() + hold;
            s.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
            let mut buf = vec![0u8; 1 << 16];
            while Instant::now() < end {
                if !read {
                    std::thread::sleep(Duration::from_millis(100));
                } else if matches!(s.read(&mut buf), Ok(0)) {
                    break;
                }
            }
        });
        (server, h)
    }

    fn put(server: &str, body: Vec<u8>) -> (Result<Vec<u8>, String>, Duration) {
        let key = tether_trust::generate_key();
        let t = Instant::now();
        let r = tauri::async_runtime::block_on(admin_upload(server, &key, "/api/admin/release/stable/x/a.exe", body, "application/octet-stream", |_| {}));
        (r, t.elapsed())
    }

    /// The relay takes the whole body and never answers: fails ANSWER after the last byte.
    #[test]
    fn upload_without_answer_fails_after_answer_limit() {
        let _one = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let (server, h) = silent(true, ANSWER + Duration::from_secs(4));
        let (r, took) = put(&server, vec![7; 600_000]);
        let e = r.unwrap_err();
        assert!(e.contains("after the upload"), "{e}");
        assert!(took >= ANSWER && took < ANSWER + Duration::from_secs(2), "{took:?}");
        h.join().unwrap();
    }

    /// The relay takes nothing (the socket buffers fill): fails STALL after the last chunk went out.
    #[test]
    fn upload_to_a_relay_that_takes_nothing_stalls() {
        let _one = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let (server, h) = silent(false, STALL + Duration::from_secs(6));
        let (r, took) = put(&server, vec![7; 64 << 20]);
        let e = r.unwrap_err();
        assert!(e.contains("stalled"), "{e}");
        // The server gives up after STALL + 6 s; a slow CI machine needs more than 3 s of slack.
        assert!(took < STALL + Duration::from_secs(5), "{took:?}");
        h.join().unwrap();
    }

    #[test]
    fn cancel_ends_an_upload_at_once() {
        let _one = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let (server, h) = silent(true, ANSWER + Duration::from_secs(2));
        begin(None);
        let c = std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(500));
            cancel(None);
        });
        let (r, took) = put(&server, vec![7; 600_000]);
        c.join().unwrap();
        assert!(cancelled());
        // A new run is not cancelled; a Cancel of a run before it began is kept.
        begin(None);
        assert!(!cancelled());
        cancel(Some(7));
        assert!(!cancelled());
        begin(Some(7));
        assert!(cancelled());
        begin(None);
        assert_eq!(r.unwrap_err(), CANCELLED);
        assert!(took < Duration::from_millis(1500), "{took:?}");
        h.join().unwrap();
    }

    /// A download whose body stops coming fails STALL after the last byte, not after its total.
    #[test]
    fn download_that_stops_sending_fails_after_stall() {
        let _one = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let server = format!("http://{}", listener.local_addr().unwrap());
        let h = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let _ = s.read(&mut buf);
            s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\n\r\nhalf").unwrap();
            std::thread::sleep(STALL + Duration::from_secs(3));
        });
        let t = Instant::now();
        let r = tauri::async_runtime::block_on(file(&server, "/download/stable/x/a.exe"));
        assert!(r.unwrap_err().contains("stopped sending"));
        assert!(t.elapsed() < STALL + Duration::from_secs(2), "{:?}", t.elapsed());
        h.join().unwrap();
    }

    /// A relay that answers with a redirect: the signed request is not sent on to the target.
    #[test]
    fn redirect_is_not_followed() {
        let target = TcpListener::bind("127.0.0.1:0").unwrap();
        target.set_nonblocking(true).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let server = format!("http://{}", listener.local_addr().unwrap());
        let to = format!("http://{}/api/admin/status", target.local_addr().unwrap());
        let h = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let _ = s.read(&mut buf);
            write!(s, "HTTP/1.1 307 X\r\nLocation: {to}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
        });
        let key = tether_trust::generate_key();
        let e = tauri::async_runtime::block_on(admin_quick(&server, &key, "/api/admin/status")).unwrap_err();
        assert!(e.contains("HTTP 307") && e.contains("redirects"), "{e}");
        h.join().unwrap();
        assert!(target.accept().is_err(), "the redirect target was contacted");
    }

    #[test]
    fn errors_are_explained() {
        let e = explain(401, "signature does not match");
        assert!(e.contains("ADMIN_PUB_KEY") && e.contains("HTTP 401: signature does not match"));
        assert!(explain(409, "published release: stable/x 1.0 is in latest.json").starts_with("The relay keeps the published release."));
        assert!(explain(409, "serial must be greater").contains("newer version"));
        // v2 signatures: each reason names what to do, not the admin key or the clock.
        let old = explain(401, "unsupported signature version");
        assert!(old.contains("Update the relay first") && !old.contains("ADMIN_PUB_KEY"), "{old}");
        let unset = explain(401, "this relay does not know its own host name (RELAY_HOST), so it cannot accept v2 signatures");
        assert!(unset.contains("Set RELAY_HOST on the relay") && !unset.contains("ADMIN_PUB_KEY"), "{unset}");
        let other = explain(401, "request signed for another relay");
        assert!(other.contains("relay's own name") && !other.contains("ADMIN_PUB_KEY"), "{other}");
        // An old relay validates the policy and does not know a newer feature (health.agent, 0.7.0).
        let feature = explain(400, "profile technician: unknown feature \"health.agent\"");
        assert!(feature.starts_with("This relay runs an older tether-api that does not know a feature of this policy") && feature.contains("Update the relay first") && feature.contains("health.agent"), "{feature}");
    }
}
