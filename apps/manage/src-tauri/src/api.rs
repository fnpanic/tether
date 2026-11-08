//! Relay API client. Every admin request is signed with `tether_trust::reqauth` over the exact
//! path (with query) and body bytes that go on the wire.

use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tether_trust::{reqauth, SigningKey};

pub fn shared() -> &'static reqwest::Client {
    client()
}

fn client() -> &'static reqwest::Client {
    static C: OnceLock<reqwest::Client> = OnceLock::new();
    C.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            // Release uploads may be up to 200 MiB on a slow uplink.
            .timeout(Duration::from_secs(30 * 60))
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
        401 => "The server did not accept this app's admin signature. Check that the server's ADMIN_PUB_KEY contains this app's admin public key and that the PC clock is correct.",
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
/// unreachable, instead of the client's 30 min upload timeout.
pub const QUICK: Duration = Duration::from_secs(10);

/// Signed admin request; returns the response body on 2xx.
pub async fn admin(server: &str, key: &SigningKey, method: &str, path: &str, body: Vec<u8>, content_type: &str) -> Result<Vec<u8>, String> {
    let m = reqwest::Method::from_bytes(method.as_bytes()).map_err(|e| e.to_string())?;
    let sig = reqauth::sign_request(key, method, path, &body, now());
    let mut req = client().request(m, url(server, path)).header(reqauth::HEADER, sig);
    if !body.is_empty() {
        req = req.header("Content-Type", content_type).body(body);
    }
    finish(req.send().await).await
}

/// Signed `GET` that gives up after [`QUICK`].
pub async fn admin_quick(server: &str, key: &SigningKey, path: &str) -> Result<Vec<u8>, String> {
    let sig = reqauth::sign_request(key, "GET", path, b"", now());
    finish(client().get(url(server, path)).header(reqauth::HEADER, sig).timeout(QUICK).send().await).await
}

/// Signed `PUT` of a large file; `sent(bytes so far)` runs as the body goes out (256 KiB steps).
pub async fn admin_upload(server: &str, key: &SigningKey, path: &str, body: Vec<u8>, content_type: &str, sent: impl Fn(u64) + Send + Sync + 'static) -> Result<Vec<u8>, String> {
    let sig = reqauth::sign_request(key, "PUT", path, &body, now());
    let len = body.len();
    let chunks: Vec<Vec<u8>> = body.chunks(256 * 1024).map(<[u8]>::to_vec).collect();
    let mut done = 0u64;
    // Lazy: a chunk is counted when the connection pulls it.
    let stream = futures_util::stream::iter(chunks.into_iter().map(move |c| {
        done += c.len() as u64;
        sent(done);
        Ok::<_, std::io::Error>(c)
    }));
    let req = client()
        .put(url(server, path))
        .header(reqauth::HEADER, sig)
        .header("Content-Type", content_type)
        // Explicit length: a streamed body would otherwise go out chunked.
        .header("Content-Length", len)
        .body(reqwest::Body::wrap_stream(stream));
    finish(req.send().await).await
}

/// Unauthenticated `HEAD` of a public file: `Ok(false)` on 404.
pub async fn exists(server: &str, path: &str) -> Result<bool, String> {
    match finish(client().head(url(server, path)).send().await).await {
        Ok(_) => Ok(true),
        Err(e) if e.contains("HTTP 404") => Ok(false),
        Err(e) => Err(e),
    }
}

/// Unauthenticated GET (public `latest.json`, `/api/policy`). `Ok(None)` on 404.
pub async fn public(server: &str, path: &str) -> Result<Option<Vec<u8>>, String> {
    public_req(client().get(url(server, path))).await
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

async fn finish(res: reqwest::Result<reqwest::Response>) -> Result<Vec<u8>, String> {
    let res = res.map_err(|e| if e.is_timeout() { "No answer from the server in time.".to_owned() } else { format!("Could not reach the server: {e}") })?;
    let status = res.status().as_u16();
    let bytes = res.bytes().await.map_err(|e| if e.is_timeout() { "The server stopped answering in time.".to_owned() } else { format!("The connection broke while reading the answer: {e}") })?;
    if (200..300).contains(&status) {
        Ok(bytes.to_vec())
    } else {
        Err(explain(status, &String::from_utf8_lossy(&bytes)))
    }
}

#[cfg(test)]
mod tests {
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
            let ok = reqauth::verify_request(&sig, &pk, &method, &path, &body, now(), |_| true);
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

    #[test]
    fn errors_are_explained() {
        let e = explain(401, "signature does not match");
        assert!(e.contains("ADMIN_PUB_KEY") && e.contains("HTTP 401: signature does not match"));
        assert!(explain(409, "published release: stable/x 1.0 is in latest.json").starts_with("The relay keeps the published release."));
        assert!(explain(409, "serial must be greater").contains("newer version"));
    }
}
