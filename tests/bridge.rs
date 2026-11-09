use relay::{bridge::Bridge, engine::Engine};
use serde_json::json;
use std::time::Duration;
use tokio::{io::AsyncWriteExt, net::TcpStream, sync::mpsc};

#[tokio::test]
async fn authentication_folder_handoff_cancellation_and_limits() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path().join("data"), dir.path().join("default")).unwrap();
    let (send, mut folders) = mpsc::unbounded_channel();
    let bridge = Bridge::start(engine.clone(), "test-token".into(), 0, send)
        .await
        .unwrap();
    assert!(bridge.address.ip().is_loopback());
    let base = format!("http://{}", bridge.address);
    let client = reqwest::Client::new();
    assert_eq!(
        client
            .get(format!("{base}/health"))
            .send()
            .await
            .unwrap()
            .status()
            .as_u16(),
        401
    );
    assert_eq!(
        client
            .get(format!("{base}/health"))
            .bearer_auth("test-token")
            .send()
            .await
            .unwrap()
            .status()
            .as_u16(),
        200
    );
    assert_eq!(
        client
            .post(format!("{base}/downloads"))
            .bearer_auth("test-token")
            .body(json!({"url":"file:///etc/passwd"}).to_string())
            .send()
            .await
            .unwrap()
            .status()
            .as_u16(),
        400
    );
    assert!(folders.try_recv().is_err());
    let post = || {
        let url = format!("{base}/downloads");
        let client = client.clone();
        tokio::spawn(async move {
            client
                .post(url)
                .bearer_auth("test-token")
                .body(json!({"url":"http://127.0.0.1:9/file.zip"}).to_string())
                .send()
                .await
                .unwrap()
        })
    };
    let pending = post();
    let request = folders.recv().await.unwrap();
    request.reply.send(None).unwrap();
    assert_eq!(pending.await.unwrap().status().as_u16(), 409);
    assert!(engine.snapshot().jobs.is_empty());
    let chosen = dir.path().join("browser selected");
    let pending = post();
    let request = folders.recv().await.unwrap();
    request.reply.send(Some(chosen.clone())).unwrap();
    assert_eq!(pending.await.unwrap().status().as_u16(), 201);
    let state = engine.snapshot();
    assert_eq!(state.jobs.len(), 1);
    assert_eq!(state.jobs[0].destination, chosen);
    assert_eq!(state.jobs[0].queue, "Browser");
    assert_eq!(state.settings.download_dir, dir.path().join("default"));
    assert_eq!(
        client
            .post(format!("{base}/downloads"))
            .bearer_auth("test-token")
            .body("x".repeat(16385))
            .send()
            .await
            .unwrap()
            .status()
            .as_u16(),
        413
    );
    engine.close().await.unwrap();
}
#[tokio::test]
async fn disconnected_browser_never_enqueues_after_folder_selection() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path().join("data"), dir.path().join("out")).unwrap();
    let (send, mut folders) = mpsc::unbounded_channel();
    let bridge = Bridge::start(engine.clone(), "token".into(), 0, send)
        .await
        .unwrap();
    let mut socket = TcpStream::connect(bridge.address).await.unwrap();
    let body = json!({"url":"http://127.0.0.1:9/file.zip"}).to_string();
    socket.write_all(format!("POST /downloads HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer token\r\nContent-Length: {}\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
    let request = folders.recv().await.unwrap();
    drop(socket);
    tokio::time::timeout(Duration::from_secs(2), async {
        while !request.reply.is_closed() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(request.reply.send(Some(dir.path().join("chosen"))).is_err());
    assert!(engine.snapshot().jobs.is_empty());
    engine.close().await.unwrap();
}
