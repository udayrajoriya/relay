use crate::{
    engine::{Engine, valid_url},
    model::AddInput,
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    net::{Ipv4Addr, SocketAddr},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};
use subtle::ConstantTimeEq;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{Semaphore, mpsc, oneshot},
};
use tokio_util::sync::CancellationToken;

pub struct FolderRequest {
    pub url: String,
    pub destination: PathBuf,
    pub reply: oneshot::Sender<Option<PathBuf>>,
}
pub struct Bridge {
    pub address: SocketAddr,
    cancel: CancellationToken,
}
impl Drop for Bridge {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
impl Bridge {
    pub async fn start(
        engine: Engine,
        token: String,
        port: u16,
        folders: mpsc::UnboundedSender<FolderRequest>,
    ) -> Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await?;
        let address = listener.local_addr()?;
        let cancel = CancellationToken::new();
        let stop = cancel.clone();
        tokio::spawn(async move {
            let slots = Arc::new(Semaphore::new(16));
            loop {
                let incoming = tokio::select! { _ = stop.cancelled() => break, incoming = listener.accept() => incoming };
                let Ok((stream, _)) = incoming else {
                    break;
                };
                let Ok(permit) = slots.clone().try_acquire_owned() else {
                    continue;
                };
                let e = engine.clone();
                let token = token.clone();
                let folders = folders.clone();
                let stop = stop.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    let _ = tokio::time::timeout(
                        Duration::from_secs(125),
                        handle(stream, e, token, folders, stop),
                    )
                    .await;
                });
            }
        });
        Ok(Self { address, cancel })
    }
}
async fn respond(stream: &mut TcpStream, status: u16, value: Value) -> Result<()> {
    let data = serde_json::to_vec(&value)?;
    let reason = match status {
        200 => "OK",
        201 => "Created",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        409 => "Conflict",
        413 => "Content Too Large",
        _ => "Error",
    };
    stream.write_all(format!("HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nCache-Control: no-store\r\nConnection: close\r\nContent-Length: {}\r\n\r\n", data.len()).as_bytes()).await?;
    stream.write_all(&data).await?;
    stream.shutdown().await?;
    Ok(())
}
async fn handle(
    mut stream: TcpStream,
    engine: Engine,
    token: String,
    folders: mpsc::UnboundedSender<FolderRequest>,
    stop: CancellationToken,
) -> Result<()> {
    let request = tokio::time::timeout(Duration::from_secs(10), read_request(&mut stream)).await;
    let (method, path, authorization, body) = match request {
        Ok(Ok(request)) => request,
        Ok(Err(error)) => {
            let status = if error.to_string().contains("too large") {
                413
            } else {
                400
            };
            return respond(&mut stream, status, json!({"error":error.to_string()})).await;
        }
        Err(_) => return respond(&mut stream, 400, json!({"error":"Request timed out."})).await,
    };
    let expected = format!("Bearer {token}");
    if !bool::from(authorization.as_bytes().ct_eq(expected.as_bytes())) {
        return respond(
            &mut stream,
            401,
            json!({"error":"Pair the extension using the token in Relay settings."}),
        )
        .await;
    }
    if method == "GET" && path == "/health" {
        return respond(
            &mut stream,
            200,
            json!({"app":"Relay","version":env!("CARGO_PKG_VERSION")}),
        )
        .await;
    }
    if method != "POST" || path != "/downloads" {
        return respond(&mut stream, 404, json!({"error":"Not found."})).await;
    }
    let input: Result<AddInput> = (|| {
        let mut input: AddInput = serde_json::from_slice(&body)?;
        valid_url(&input.url)?;
        input.queue = Some("Browser".into());
        input.connections = None;
        input.paused = false;
        input.scheduled_at = None;
        Ok(input)
    })();
    let mut input = match input {
        Ok(input) => input,
        Err(error) => return respond(&mut stream, 400, json!({"error":error.to_string()})).await,
    };
    let (reply, mut selected) = oneshot::channel();
    folders
        .send(FolderRequest {
            url: input.url.clone(),
            destination: engine.snapshot().settings.download_dir,
            reply,
        })
        .context("The desktop is closing")?;
    // Read after the complete request body to detect a browser that timed out or disconnected while the folder picker was open.
    let mut extra = [0_u8; 1];
    let destination = tokio::select! {
        _ = stop.cancelled() => return Ok(()),
        _ = stream.read(&mut extra) => return Ok(()),
        result = &mut selected => result.unwrap_or(None),
    };
    let Some(destination) = destination else {
        return respond(
            &mut stream,
            409,
            json!({"error":"Download canceled in Relay. The browser download was kept."}),
        )
        .await;
    };
    if stream.try_read(&mut extra).is_ok() {
        return Ok(());
    }
    input.destination = Some(destination);
    match engine.add(input) {
        Ok(id) => respond(&mut stream, 201, json!({"id":id})).await,
        Err(error) => respond(&mut stream, 400, json!({"error":error.to_string()})).await,
    }
}
async fn read_request(stream: &mut TcpStream) -> Result<(String, String, String, Vec<u8>)> {
    let mut data = Vec::new();
    let mut buffer = [0_u8; 2048];
    let (method, path, authorization, length, header_end) = loop {
        ensure!(data.len() < 32 * 1024, "Request too large.");
        let n = stream.read(&mut buffer).await?;
        ensure!(n > 0, "Incomplete request.");
        data.extend_from_slice(&buffer[..n]);
        let mut headers = [httparse::EMPTY_HEADER; 32];
        let mut request = httparse::Request::new(&mut headers);
        if let httparse::Status::Complete(end) = request.parse(&data)? {
            ensure!(
                !request
                    .headers
                    .iter()
                    .any(|h| h.name.eq_ignore_ascii_case("transfer-encoding")),
                "Chunked requests are not supported."
            );
            let lengths: Vec<_> = request
                .headers
                .iter()
                .filter(|h| h.name.eq_ignore_ascii_case("content-length"))
                .collect();
            ensure!(lengths.len() <= 1, "Ambiguous content length.");
            let length: usize = lengths
                .first()
                .map(|h| -> Result<usize> { Ok(std::str::from_utf8(h.value)?.parse::<usize>()?) })
                .transpose()?
                .unwrap_or(0);
            ensure!(length <= 16384, "Request too large.");
            let auth = request
                .headers
                .iter()
                .find(|h| h.name.eq_ignore_ascii_case("authorization"))
                .map(|h| String::from_utf8_lossy(h.value).into_owned())
                .unwrap_or_default();
            break (
                request.method.unwrap_or("").into(),
                request.path.unwrap_or("").into(),
                auth,
                length,
                end,
            );
        }
    };
    while data.len() < header_end + length {
        let n = stream.read(&mut buffer).await?;
        ensure!(n > 0, "Incomplete request body.");
        data.extend_from_slice(&buffer[..n]);
    }
    Ok((
        method,
        path,
        authorization,
        data[header_end..header_end + length].to_vec(),
    ))
}
