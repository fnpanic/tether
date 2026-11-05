//! turn-bench <turn|tcp> <seconds>
//! "Host" sends screen frames at 30 fps (12 KB, an 80 KB keyframe every 2 s) and the "viewer"
//! sends 50-byte input events at 60 Hz, through
//!   turn: the core's WebRTCStream (KCP-style SCTP) with the host relay-only via hbbs's TURN
//!         (vendor/rustdesk-server/src/turn.rs, single port, as in production);
//!   tcp:  a TCP relay that pairs two connections and copies bytes, like hbbr.
//! Run it under netem (lo) in a network namespace; prints one line of latency/stall stats.

#[path = "../../../vendor/rustdesk-server/src/turn.rs"]
#[allow(dead_code)]
mod turn_srv;

use hbb_common::webrtc::WebRTCStream;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};

static EPOCH: once_cell::sync::Lazy<Instant> = once_cell::sync::Lazy::new(Instant::now);
fn now_us() -> u64 {
    EPOCH.elapsed().as_micros() as u64
}

fn msg(seq: u32, len: usize) -> Vec<u8> {
    let mut m = vec![0u8; len.max(12)];
    m[..8].copy_from_slice(&now_us().to_le_bytes());
    m[8..12].copy_from_slice(&seq.to_le_bytes());
    m
}

#[derive(Default)]
struct Stats {
    lat: Vec<u64>,
    arrivals: Vec<u64>,
}

impl Stats {
    fn add(&mut self, m: &[u8]) {
        let t = now_us();
        let sent = u64::from_le_bytes(m[..8].try_into().unwrap());
        self.lat.push(t - sent);
        self.arrivals.push(t);
    }
    fn summary(&self, name: &str, gap_ms: u64) -> String {
        let mut l = self.lat.clone();
        l.sort();
        let p = |q: f64| l.get(((l.len() as f64 - 1.) * q) as usize).map_or(0., |v| *v as f64 / 1000.);
        let gaps: Vec<u64> = self.arrivals.windows(2).map(|w| w[1] - w[0]).collect();
        let stalls = gaps.iter().filter(|g| **g > gap_ms * 1000).count();
        let max_gap = gaps.iter().max().copied().unwrap_or(0) as f64 / 1000.;
        format!(
            "{name}: n={} p50={:.1} p95={:.1} p99={:.1} max={:.1} ms, gaps>{gap_ms}ms={stalls} maxgap={max_gap:.0}ms",
            l.len(),
            p(0.5),
            p(0.95),
            p(0.99),
            p(1.0)
        )
    }
}

/// Frames host->viewer and input viewer->host for `secs`, over any message transport.
async fn run<S, R>(host_tx: S, viewer_tx: S, host_rx: R, viewer_rx: R, secs: u64) -> String
where
    S: FnMut(Vec<u8>) -> futures_like::BoxFut + Send + 'static,
    R: FnMut() -> futures_like::BoxRecv + Send + 'static,
{
    let frames = Arc::new(Mutex::new(Stats::default()));
    let input = Arc::new(Mutex::new(Stats::default()));
    let end = Instant::now() + Duration::from_secs(secs);
    let mut host_tx = host_tx;
    let mut viewer_tx = viewer_tx;
    let send_frames = tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_micros(33_333));
        let mut seq = 0u32;
        while Instant::now() < end {
            tick.tick().await;
            let len = if seq % 60 == 0 { 80_000 } else { 12_000 };
            if host_tx(msg(seq, len)).await.is_err() {
                break;
            }
            seq += 1;
        }
    });
    let send_input = tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_micros(16_667));
        let mut seq = 0u32;
        while Instant::now() < end {
            tick.tick().await;
            if viewer_tx(msg(seq, 50)).await.is_err() {
                break;
            }
            seq += 1;
        }
    });
    let f = frames.clone();
    let mut viewer_rx = viewer_rx;
    let recv_frames = tokio::spawn(async move {
        while let Ok(Some(m)) = tokio::time::timeout_at((end + Duration::from_secs(3)).into(), viewer_rx()).await {
            f.lock().unwrap().add(&m);
        }
    });
    let i = input.clone();
    let mut host_rx = host_rx;
    let recv_input = tokio::spawn(async move {
        while let Ok(Some(m)) = tokio::time::timeout_at((end + Duration::from_secs(3)).into(), host_rx()).await {
            i.lock().unwrap().add(&m);
        }
    });
    let _ = tokio::join!(send_frames, send_input, recv_frames, recv_input);
    let s = format!("{} | {}", frames.lock().unwrap().summary("frames", 100), input.lock().unwrap().summary("input", 50));
    s
}

mod futures_like {
    use std::future::Future;
    use std::pin::Pin;
    pub type BoxFut = Pin<Box<dyn Future<Output = Result<(), ()>> + Send>>;
    pub type BoxRecv = Pin<Box<dyn Future<Output = Option<Vec<u8>>> + Send>>;
}
use futures_like::{BoxFut, BoxRecv};

fn webrtc_tx(s: &WebRTCStream) -> impl FnMut(Vec<u8>) -> BoxFut {
    let s = s.clone();
    move |m| {
        let mut s = s.clone();
        Box::pin(async move { s.send_raw(m).await.map_err(|_| ()) })
    }
}

fn webrtc_rx(s: &WebRTCStream) -> impl FnMut() -> BoxRecv {
    let s = s.clone();
    move || {
        let mut s = s.clone();
        Box::pin(async move { s.next().await.and_then(|r| r.ok()).map(|b| b.to_vec()) })
    }
}

async fn turn_path(secs: u64) -> String {
    let hbbs = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let port = hbbs.local_addr().unwrap().port();
    let t = turn_srv::Turn::start(turn_srv::TurnConfig {
        host: "127.0.0.1".into(),
        port,
        public_ip: Some(std::net::Ipv4Addr::LOCALHOST),
        max_allocations: 4,
        bandwidth: 64 << 20,
        allow_private: true,
    })
    .await
    .unwrap();
    t.set_socket(&hbbs);
    let t2 = t.clone();
    tokio::spawn(async move {
        let mut buf = vec![0u8; 4096];
        while let Ok((n, from)) = hbbs.recv_from(&mut buf).await {
            t2.dispatch(&buf[..n], from);
        }
    });
    let url = t.url("bench");
    let t0 = Instant::now();
    let mut viewer = WebRTCStream::new("", false, 20000).await.unwrap();
    let offer = viewer.local_endpoint().to_owned();
    // The host is relay-only, so every byte crosses the TURN server.
    let host = WebRTCStream::new_with_turn(&offer, true, 20000, &url).await.unwrap();
    viewer.set_remote_endpoint(host.local_endpoint()).await.unwrap();
    let (mut vi, mut hi) = (viewer.take_local_ice_rx().unwrap(), host.take_local_ice_rx().unwrap());
    let (h2, v2) = (host.clone(), viewer.clone());
    tokio::spawn(async move {
        while let Some(c) = vi.recv().await {
            let _ = h2.add_remote_ice_candidate(&c).await;
        }
    });
    tokio::spawn(async move {
        while let Some(c) = hi.recv().await {
            let _ = v2.add_remote_ice_candidate(&c).await;
        }
    });
    viewer.wait_connected(20000).await.unwrap();
    let mut h3 = host.clone();
    h3.wait_connected(20000).await.unwrap();
    let setup = t0.elapsed().as_millis();
    let relayed = viewer.is_relayed().await;
    let r = run(webrtc_tx(&host), webrtc_tx(&viewer), webrtc_rx(&host), webrtc_rx(&viewer), secs).await;
    format!("turn (relayed={relayed:?}, connected in {setup} ms) {r}")
}

async fn read_frame(s: &mut tokio::net::tcp::OwnedReadHalf) -> Option<Vec<u8>> {
    let n = s.read_u32().await.ok()? as usize;
    let mut b = vec![0u8; n];
    s.read_exact(&mut b).await.ok()?;
    Some(b)
}

fn tcp_tx(w: tokio::net::tcp::OwnedWriteHalf) -> impl FnMut(Vec<u8>) -> BoxFut {
    let w = Arc::new(tokio::sync::Mutex::new(w));
    move |m| {
        let w = w.clone();
        Box::pin(async move {
            let mut w = w.lock().await;
            let mut b = (m.len() as u32).to_be_bytes().to_vec();
            b.extend(m);
            w.write_all(&b).await.map_err(|_| ())
        })
    }
}

fn tcp_rx(r: tokio::net::tcp::OwnedReadHalf) -> impl FnMut() -> BoxRecv {
    let r = Arc::new(tokio::sync::Mutex::new(r));
    move || {
        let r = r.clone();
        Box::pin(async move { read_frame(&mut *r.lock().await).await })
    }
}

/// RELAY_CC=bbr: the relay's own sockets use that congestion control (production: BBR on the
/// relays, the clients' OS default, CUBIC, on their side), the rest the namespace default.
#[cfg(target_os = "linux")]
fn set_cc(s: &TcpStream, cc: &str) {
    use std::os::fd::AsRawFd;
    let r = unsafe {
        libc::setsockopt(s.as_raw_fd(), libc::IPPROTO_TCP, libc::TCP_CONGESTION, cc.as_ptr() as *const _, cc.len() as _)
    };
    assert_eq!(r, 0, "TCP_CONGESTION {cc}");
}
#[cfg(not(target_os = "linux"))]
fn set_cc(_: &TcpStream, _: &str) {}

async fn tcp_path(secs: u64) -> String {
    let relay = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = relay.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut a, _) = relay.accept().await.unwrap();
        let (mut b, _) = relay.accept().await.unwrap();
        a.set_nodelay(true).ok();
        b.set_nodelay(true).ok();
        if let Ok(cc) = std::env::var("RELAY_CC") {
            set_cc(&a, &cc);
            set_cc(&b, &cc);
        }
        let _ = tokio::io::copy_bidirectional(&mut a, &mut b).await;
    });
    let host = TcpStream::connect(addr).await.unwrap();
    let viewer = TcpStream::connect(addr).await.unwrap();
    host.set_nodelay(true).ok();
    viewer.set_nodelay(true).ok();
    let (hr, hw) = host.into_split();
    let (vr, vw) = viewer.into_split();
    let r = run(tcp_tx(hw), tcp_tx(vw), tcp_rx(hr), tcp_rx(vr), secs).await;
    format!("tcp (relay cc {}) {r}", std::env::var("RELAY_CC").unwrap_or("default".into()))
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let _ = *EPOCH;
    let args: Vec<String> = std::env::args().collect();
    let secs = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(20);
    let line = match args.get(1).map(|s| s.as_str()) {
        Some("turn") => turn_path(secs).await,
        Some("tcp") => tcp_path(secs).await,
        _ => {
            eprintln!("usage: turn-bench <turn|tcp> [seconds]");
            std::process::exit(2)
        }
    };
    println!("{line}");
    std::process::exit(0);
}
