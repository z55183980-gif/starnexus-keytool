//! Exercises the real updater's HTTP/download/signature path without running an installer.
use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
    time::{Duration, Instant},
};
use tauri::test::{mock_builder, mock_context, noop_assets};
use tauri_plugin_updater::UpdaterExt;

const PAYLOAD: &[u8] = include_bytes!("fixtures/update-payload.txt");
const SIGNATURE: &str = include_str!("fixtures/update-payload.txt.sig");
const PUBLIC_KEY: &str = include_str!("fixtures/update-pubkey.txt");

fn serve(tampered: bool, manifest_status: u16) -> (String, thread::JoinHandle<usize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let manifest = serde_json::json!({
        "version": "99.0.0", "url": format!("{base}/package"), "signature": SIGNATURE.trim(),
        "notes": "Fixture, never install", "pub_date": "2026-11-14T00:00:00Z"
    })
    .to_string();
    let handle = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut requests = 0;
        let expected = if manifest_status == 200 { 2 } else { 1 };
        while requests < expected && Instant::now() < deadline {
            let (mut stream, _) = match listener.accept() {
                Ok(pair) => pair,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                    continue;
                }
                Err(e) => panic!("{e}"),
            };
            // Accepted Windows sockets inherit the listener's nonblocking mode.
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 1024];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = stream.read(&mut buffer).unwrap();
                assert_ne!(n, 0);
                request.extend_from_slice(&buffer[..n]);
            }
            let (status, body) = if request.starts_with(b"GET /package ") {
                (
                    200,
                    if tampered {
                        b"tampered archive".to_vec()
                    } else {
                        PAYLOAD.to_vec()
                    },
                )
            } else {
                (
                    manifest_status,
                    if manifest_status == 200 {
                        manifest.as_bytes().to_vec()
                    } else {
                        Vec::new()
                    },
                )
            };
            write!(stream, "HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
            stream.write_all(&body).unwrap();
            requests += 1;
        }
        requests
    });
    (base, handle)
}

#[test]
fn actual_plugin_download_accepts_signed_payload_and_rejects_tampering() {
    for tampered in [false, true] {
        let (base, server) = serve(tampered, 200);
        let mut context = mock_context(noop_assets());
        context.config_mut().plugins.0.insert(
            "updater".into(),
            serde_json::json!({
                "pubkey": PUBLIC_KEY.trim(), "dangerousInsecureTransportProtocol": true,
            }),
        );
        let app = mock_builder()
            .plugin(tauri_plugin_updater::Builder::new().build())
            .build(context)
            .unwrap();
        tauri::async_runtime::block_on(async {
            let updater = app
                .updater_builder()
                .endpoints(vec![format!("{base}/latest.json").parse().unwrap()])
                .unwrap()
                .no_proxy()
                .timeout(Duration::from_secs(3))
                .build()
                .unwrap();
            let update = updater.check().await.unwrap().unwrap();
            assert_eq!(update.version, "99.0.0");
            let mut received = 0;
            let result = update.download(|n, _| received += n, || {}).await;
            assert!(received > 0);
            if tampered {
                assert!(result.is_err(), "tampered bytes must never reach install");
            } else {
                assert_eq!(result.unwrap(), PAYLOAD);
            }
        });
        assert_eq!(server.join().unwrap(), 2);
    }
}

#[test]
fn actual_plugin_distinguishes_no_update_from_unavailable_service() {
    for status in [204, 404] {
        let (base, server) = serve(false, status);
        let mut context = mock_context(noop_assets());
        context.config_mut().plugins.0.insert(
            "updater".into(),
            serde_json::json!({
                "pubkey": PUBLIC_KEY.trim(), "dangerousInsecureTransportProtocol": true,
            }),
        );
        let app = mock_builder()
            .plugin(tauri_plugin_updater::Builder::new().build())
            .build(context)
            .unwrap();
        tauri::async_runtime::block_on(async {
            let updater = app
                .updater_builder()
                .endpoints(vec![base.parse().unwrap()])
                .unwrap()
                .no_proxy()
                .timeout(Duration::from_secs(3))
                .build()
                .unwrap();
            let result = updater.check().await;
            if status == 204 {
                assert!(result.unwrap().is_none());
            } else {
                assert!(result.is_err());
            }
        });
        assert_eq!(server.join().unwrap(), 1);
    }
}
