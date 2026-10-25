use std::{io::Cursor, process::Stdio, sync::Arc, time::Duration};

use base64::{engine::general_purpose::STANDARD, Engine};

use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, BufReader},
    net::TcpListener,
    process::Command,
    time::timeout,
};

#[tokio::test]
async fn secure_websocket_sends_tls_client_hello() {
    let root = std::env::temp_dir().join(format!("apeiron-tls-{}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    let ca_dir = root.join("ca");
    std::fs::create_dir(&ca_dir).unwrap();
    let ca_file = ca_dir.join("fixture.pem");
    std::fs::write(
        &ca_file,
        format!(
            "-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n",
            STANDARD.encode(include_bytes!("fixtures/tls/untrusted-cert.der"))
        ),
    )
    .unwrap();
    let empty = root.join("empty");
    std::fs::create_dir(&empty).unwrap();
    std::fs::write(empty.join("empty.pem"), b"").unwrap();
    for source in ["system", "file", "directory", "both", "empty file", "empty directory"] {
        check_source(&root.join(source), source, &ca_file, &ca_dir, &empty).await;
    }
    std::fs::remove_dir_all(root).unwrap();
}

async fn check_source(
    root: &std::path::Path,
    source: &str,
    ca_file: &std::path::Path,
    ca_dir: &std::path::Path,
    empty: &std::path::Path,
) {
    std::fs::create_dir(root).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_apeiron"));
    command
        .args([
            "serve",
            "--server",
            &format!("https://{}", listener.local_addr().unwrap()),
        ])
        .args(["--token", "fixture-token", "--root"])
        .arg(root.join("workspace"))
        .arg("--config-home")
        .arg(root.join("config"))
        .env("HOME", root)
        .env_remove("APEIRON_TOKEN_FILE")
        .env_remove("SSL_CERT_FILE")
        .env_remove("SSL_CERT_DIR")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    match source {
        "file" => {
            command.env("SSL_CERT_FILE", ca_file);
        }
        "directory" => {
            command.env("SSL_CERT_DIR", std::env::join_paths([empty, ca_dir]).unwrap());
        }
        "both" => {
            command
                .env("SSL_CERT_FILE", empty.join("empty.pem"))
                .env("SSL_CERT_DIR", ca_dir);
        }
        "empty file" => {
            command.env("SSL_CERT_FILE", empty.join("empty.pem"));
        }
        "empty directory" => {
            command.env("SSL_CERT_DIR", empty);
        }
        _ => {}
    }
    let trusted = matches!(source, "file" | "directory" | "both");
    let no_roots = source.starts_with("empty");
    let mut client = command.spawn().unwrap();

    timeout(Duration::from_secs(10), async {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut record = [0; 3];
        if no_roots {
            // An explicit empty trust store must not fall back to OS roots.
            assert!(socket.read_exact(&mut record).await.is_err());
            return;
        }
        socket.read_exact(&mut record).await.unwrap();
        assert_eq!(record[..2], [0x16, 0x03], "expected a TLS handshake record");
        tokio::task::spawn_blocking(move || {
            // Public fixture CA/key, also used to sign this test-only leaf.
            // No test certificate is ever installed in the OS trust store.
            let config = rustls::ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(
                    vec![include_bytes!("fixtures/tls/server-cert.der").to_vec().into()],
                    rustls::pki_types::PrivatePkcs8KeyDer::from(
                        include_bytes!("fixtures/tls/untrusted-key.der").to_vec(),
                    )
                    .into(),
                )
                .unwrap();
            let mut server = rustls::ServerConnection::new(Arc::new(config)).unwrap();
            server.read_tls(&mut Cursor::new(record)).unwrap();
            let mut socket = socket.into_std().unwrap();
            socket.set_nonblocking(false).unwrap();
            socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            socket.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
            if trusted {
                server.complete_io(&mut socket).expect("explicit CA trusts the server");
                let mut websocket = tokio_tungstenite::tungstenite::accept(rustls::StreamOwned::new(server, socket))
                    .expect("trusted TLS completes the WebSocket upgrade");
                websocket
                    .close(Some(tokio_tungstenite::tungstenite::protocol::CloseFrame {
                        code: 4002.into(),
                        reason: "trusted fixture".into(),
                    }))
                    .unwrap();
                return;
            }
            let error = server
                .complete_io(&mut socket)
                .expect_err("CLI must reject an untrusted certificate");
            assert!(matches!(
                error.get_ref().and_then(|error| error.downcast_ref::<rustls::Error>()),
                Some(rustls::Error::AlertReceived(_))
            ));
        })
        .await
        .unwrap();
    })
    .await
    .unwrap_or_else(|_| panic!("TLS fixture timed out: {source}"));

    if trusted {
        assert!(timeout(Duration::from_secs(5), client.wait())
            .await
            .unwrap()
            .unwrap()
            .success());
    } else {
        timeout(Duration::from_secs(5), async {
            let mut stderr = BufReader::new(client.stderr.take().unwrap()).lines();
            loop {
                let line = stderr.next_line().await.unwrap().expect("CLI reports the TLS failure");
                if line.contains("connection failed") {
                    assert!(line.to_lowercase().contains("certificate"), "{line}");
                    break;
                }
            }
        })
        .await
        .unwrap();
        client.kill().await.unwrap();
        client.wait().await.unwrap();
    }
}
