use std::time::Duration;

use tokio::{io::AsyncReadExt, net::TcpListener, time::timeout};
use tokio_tungstenite::connect_async;

#[tokio::test]
async fn secure_websocket_sends_tls_client_hello() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let client = tokio::spawn(async move { connect_async(format!("wss://{address}/local/connect")).await });

    timeout(Duration::from_secs(5), async {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut record = [0; 3];
        socket.read_exact(&mut record).await.unwrap();
        assert_eq!(record[..2], [0x16, 0x03], "expected a TLS handshake record");
    })
    .await
    .expect("the client must start TLS before waiting for the server");

    // Closing the test peer fails the handshake normally, without crashing the client.
    assert!(timeout(Duration::from_secs(5), client)
        .await
        .expect("the closed peer must end the handshake")
        .expect("TLS setup must not panic")
        .is_err());
}
