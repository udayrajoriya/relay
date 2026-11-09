use relay::{
    engine::{Engine, safe_name, valid_url},
    model::{AddInput, Job},
    profile::{self, ProfileLock},
};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use tokio_util::sync::CancellationToken;

const SIZE: usize = 3 * 1024 * 1024 + 137;
fn payload(version: usize) -> Vec<u8> {
    (0..SIZE)
        .map(|i| ((i * 31 + i % 251 + version) % 256) as u8)
        .collect()
}
fn hash(bytes: &[u8]) -> Vec<u8> {
    Sha256::digest(bytes).to_vec()
}
#[derive(Clone, Debug)]
struct Request {
    route: String,
    start: Option<usize>,
    end: Option<usize>,
}
struct Fixture {
    root: tempfile::TempDir,
    base: String,
    requests: Arc<Mutex<Vec<Request>>>,
    version: Arc<AtomicUsize>,
    maximum: Arc<AtomicUsize>,
    cancel: CancellationToken,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
impl Fixture {
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let version = Arc::new(AtomicUsize::new(0));
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let broken = Arc::new(AtomicBool::new(false));
        let cancel = CancellationToken::new();
        let stop = cancel.clone();
        let records = requests.clone();
        let v = version.clone();
        let max = maximum.clone();
        tokio::spawn(async move {
            loop {
                let incoming = tokio::select! { _ = stop.cancelled() => break, incoming = listener.accept() => incoming };
                let Ok((mut stream, _)) = incoming else {
                    break;
                };
                let records = records.clone();
                let version = v.clone();
                let active = active.clone();
                let maximum = max.clone();
                let broken = broken.clone();
                tokio::spawn(async move {
                    let _ = async {
                    let mut data = Vec::new(); let mut buffer = [0; 4096];
                    while !data.windows(4).any(|w| w == b"\r\n\r\n") { let n = stream.read(&mut buffer).await?; if n == 0 { return Ok::<(), std::io::Error>(()); } data.extend_from_slice(&buffer[..n]); }
                    let text = String::from_utf8_lossy(&data); let route = text.split_whitespace().nth(1).unwrap_or("/").to_string();
                    let range = text.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("range: bytes=").map(str::to_string));
                    let (start, end) = range.as_deref().and_then(|r| r.split_once('-')).and_then(|(s,e)| Some((s.parse::<usize>().ok()?, e.trim().parse::<usize>().ok()?))).map(|(s,e)| (Some(s),Some(e))).unwrap_or((None,None));
                    records.lock().unwrap().push(Request { route: route.clone(), start, end });
                    if route == "/redirect" { stream.write_all(b"HTTP/1.1 302 Found\r\nLocation: /file\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await?; return Ok(()); }
                    if route == "/unsafe-redirect" { stream.write_all(b"HTTP/1.1 302 Found\r\nLocation: ftp://example.org/file\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await?; return Ok(()); }
                    if route == "/error" { stream.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await?; return Ok(()); }
                    if route == "/empty" { let response: &[u8] = if start.is_some() { b"HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */0\r\nContent-Length: 0\r\nConnection: close\r\n\r\n" } else { b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n" }; stream.write_all(response).await?; return Ok(()); }
                    let version = version.load(Ordering::SeqCst); let bytes = payload(version); let size = if route == "/small" { 128 * 1024 } else { SIZE };
                    let partial = start.is_some() && !["/plain", "/unknown"].contains(&route.as_str()) && !(route == "/liar" && end != Some(0));
                    let from = if partial { start.unwrap() } else { 0 }; let to = if partial { end.unwrap().min(size - 1) } else { size - 1 };
                    let mut headers = format!("HTTP/1.1 {}\r\nConnection: close\r\nContent-Disposition: attachment; filename*=UTF-8''test%20file.zip\r\n", if partial { "206 Partial Content" } else { "200 OK" });
                    if route != "/no-validator" { headers += &format!("ETag: \"v{version}\"\r\n"); }
                    if partial { headers += &format!("Content-Range: bytes {from}-{to}/{size}\r\n"); }
                    if route == "/unknown" { headers += "Transfer-Encoding: chunked\r\n"; } else { headers += &format!("Content-Length: {}\r\n", to - from + 1); }
                    if route == "/compressed" && end != Some(0) { headers += "Content-Encoding: gzip\r\n"; }
                    headers += "\r\n"; stream.write_all(headers.as_bytes()).await?;
                    let count = active.fetch_add(1, Ordering::SeqCst) + 1; maximum.fetch_max(count, Ordering::SeqCst);
                    let result: Result<(), std::io::Error> = async {
                        let mut pos = from;
                        while pos <= to {
                            if route == "/flaky" && end != Some(0) && pos > from + 32000 && !broken.swap(true, Ordering::SeqCst) { break; }
                            let next = (pos + 16384).min(to + 1);
                            if route == "/unknown" { stream.write_all(format!("{:x}\r\n", next-pos).as_bytes()).await?; }
                            stream.write_all(&bytes[pos..next]).await?;
                            if route == "/unknown" { stream.write_all(b"\r\n").await?; }
                            pos = next; tokio::time::sleep(Duration::from_millis(3)).await;
                        }
                        if route == "/unknown" { stream.write_all(b"0\r\n\r\n").await?; } Ok(())
                    }.await;
                    active.fetch_sub(1, Ordering::SeqCst); result
                }.await;
                });
            }
        });
        Self {
            root,
            base,
            requests,
            version,
            maximum,
            cancel,
        }
    }
    fn engine(&self) -> Engine {
        Engine::open(self.root.path().join("data"), self.root.path().join("out")).unwrap()
    }
    fn input(&self, route: &str) -> AddInput {
        AddInput {
            url: format!("{}{route}", self.base),
            ..Default::default()
        }
    }
}
async fn until(mut f: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(15), async {
        while !f() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("timed out");
}
async fn finish(e: &Engine, id: &str) -> Job {
    until(|| matches!(e.get(id).unwrap().status.as_str(), "completed" | "error")).await;
    let j = e.get(id).unwrap();
    assert_eq!(j.status, "completed", "{:?}", j.error);
    j
}
fn integrity(j: &Job, data: &[u8]) {
    assert_eq!(
        hash(&fs::read(j.file_path.as_ref().unwrap()).unwrap()),
        hash(data)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn segments_redirect_integrity_and_organization() {
    let f = Fixture::new().await;
    let e = f.engine();
    let j = finish(&e, &e.add(f.input("/redirect")).unwrap()).await;
    integrity(&j, &payload(0));
    assert_eq!(j.name, "test file.zip");
    assert_eq!(j.category, "Archives");
    assert!(j.parts.len() > 6);
    assert!(
        f.requests
            .lock()
            .unwrap()
            .iter()
            .any(|r| r.route == "/file" && r.end.is_some_and(|end| end > 0))
    );
    assert!(f.maximum.load(Ordering::SeqCst) > 1);
    e.close().await.unwrap();
}
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn pause_resume_and_existing_files() {
    let f = Fixture::new().await;
    let e = f.engine();
    let mut input = f.input("/file");
    input.name = Some("same.zip".into());
    let id = e.add(input.clone()).unwrap();
    until(|| e.get(&id).unwrap().received > 200000).await;
    e.action(&id, "pause").await.unwrap();
    let paused = e.get(&id).unwrap();
    let p = paused
        .parts
        .iter()
        .find(|p| p.done > 0 && p.done < p.length().unwrap())
        .unwrap();
    let from = (p.start + p.done) as usize;
    let before = f.requests.lock().unwrap().len();
    e.action(&id, "resume").await.unwrap();
    let j = finish(&e, &id).await;
    integrity(&j, &payload(0));
    assert!(
        f.requests.lock().unwrap()[before..]
            .iter()
            .any(|r| r.start == Some(from))
    );
    let next = finish(&e, &e.add(input).unwrap()).await;
    assert_ne!(next.file_path, j.file_path);
    integrity(&j, &payload(0));
    e.close().await.unwrap();
}
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn custom_destination_and_restart_recount_disk_bytes() {
    let f = Fixture::new().await;
    let e = f.engine();
    let chosen = f.root.path().join("chosen folder");
    let mut input = f.input("/file");
    input.destination = Some(chosen.clone());
    let id = e.add(input).unwrap();
    until(|| e.get(&id).unwrap().received > 200000).await;
    e.close().await.unwrap();
    assert!(chosen.join(".relay-parts").join(&id).exists());
    assert!(!f.root.path().join("out").exists());
    let restored = f.engine();
    assert_eq!(restored.get(&id).unwrap().status, "paused");
    assert!(restored.get(&id).unwrap().received > 0);
    let mut settings = restored.snapshot().settings;
    settings.download_dir = f.root.path().join("new default");
    restored.settings(settings).unwrap();
    restored.action(&id, "resume").await.unwrap();
    let j = finish(&restored, &id).await;
    assert!(j.file_path.as_ref().unwrap().starts_with(&chosen));
    integrity(&j, &payload(0));
    assert!(!chosen.join(".relay-parts").exists());
    restored.close().await.unwrap();
}
async fn legacy(f: &Fixture) -> (String, PathBuf) {
    let e = f.engine();
    let id = e.add(f.input("/file")).unwrap();
    until(|| e.get(&id).unwrap().received > 200000).await;
    e.close().await.unwrap();
    let j = e.get(&id).unwrap();
    let source = j.destination.join(".relay-parts").join(&id);
    let target = f.root.path().join("data/parts").join(&id);
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::rename(source, &target).unwrap();
    let path = f.root.path().join("data/state.json");
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    value["jobs"][0]
        .as_object_mut()
        .unwrap()
        .remove("partsInDestination");
    fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
    (id, target)
}
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn legacy_parts_migration_preserves_progress() {
    let f = Fixture::new().await;
    let (id, original) = legacy(&f).await;
    let e = f.engine();
    let received = e.get(&id).unwrap().received;
    assert!(received > 0);
    e.action(&id, "resume").await.unwrap();
    integrity(&finish(&e, &id).await, &payload(0));
    assert!(!original.exists());
    e.close().await.unwrap();
}
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn failed_migration_keeps_original_bytes_for_retry() {
    let f = Fixture::new().await;
    let (id, original) = legacy(&f).await;
    let block = f.root.path().join("unavailable");
    fs::write(&block, b"blocked").unwrap();
    let path = f.root.path().join("data/state.json");
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    value["jobs"][0]["destination"] = serde_json::json!(block);
    fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
    let e = f.engine();
    e.action(&id, "resume").await.unwrap();
    until(|| e.get(&id).unwrap().status == "error").await;
    assert!(original.exists());
    assert!(!e.get(&id).unwrap().parts_in_destination);
    fs::remove_file(block).unwrap();
    e.action(&id, "resume").await.unwrap();
    integrity(&finish(&e, &id).await, &payload(0));
    e.close().await.unwrap();
}
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn changed_validator_restarts_bytes() {
    let f = Fixture::new().await;
    let e = f.engine();
    let id = e.add(f.input("/file")).unwrap();
    until(|| e.get(&id).unwrap().received > 200000).await;
    e.action(&id, "pause").await.unwrap();
    f.version.store(1, Ordering::SeqCst);
    e.action(&id, "resume").await.unwrap();
    integrity(&finish(&e, &id).await, &payload(1));
    e.close().await.unwrap();
}
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn missing_validator_and_no_ranges_restart_safely() {
    for route in ["/plain", "/no-validator"] {
        let f = Fixture::new().await;
        let e = f.engine();
        let id = e.add(f.input(route)).unwrap();
        until(|| e.get(&id).unwrap().received > 100000).await;
        e.action(&id, "pause").await.unwrap();
        e.action(&id, "resume").await.unwrap();
        integrity(&finish(&e, &id).await, &payload(0));
        e.close().await.unwrap();
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn unknown_length_and_empty_files() {
    let f = Fixture::new().await;
    let e = f.engine();
    let j = finish(&e, &e.add(f.input("/unknown")).unwrap()).await;
    assert_eq!(j.size, Some(SIZE as u64));
    integrity(&j, &payload(0));
    let empty = finish(&e, &e.add(f.input("/empty")).unwrap()).await;
    integrity(&empty, &[]);
    e.close().await.unwrap();
}
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn disconnected_segment_is_retried() {
    let f = Fixture::new().await;
    let e = f.engine();
    integrity(
        &finish(&e, &e.add(f.input("/flaky")).unwrap()).await,
        &payload(0),
    );
    e.close().await.unwrap();
}
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn invalid_http_ranges_encoding_and_redirects_are_errors() {
    let f = Fixture::new().await;
    let e = f.engine();
    for route in ["/error", "/liar", "/compressed", "/unsafe-redirect"] {
        let id = e.add(f.input(route)).unwrap();
        until(|| e.get(&id).unwrap().status == "error").await;
        let j = e.get(&id).unwrap();
        assert!(j.error.is_some());
        assert!(j.file_path.is_none());
    }
    e.close().await.unwrap();
}
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn scheduling_and_queue_order() {
    let f = Fixture::new().await;
    let e = f.engine();
    let mut input = f.input("/small");
    let at = chrono::Utc::now() + chrono::Duration::milliseconds(700);
    input.scheduled_at = Some(at);
    let id = e.add(input).unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(e.get(&id).unwrap().status, "queued");
    assert!(f.requests.lock().unwrap().is_empty());
    assert!(finish(&e, &id).await.completed_at.unwrap() >= at);
    let a = e.add(f.input("/file")).unwrap();
    let b = e.add(f.input("/small")).unwrap();
    until(|| e.get(&a).unwrap().status == "downloading").await;
    assert_eq!(e.get(&b).unwrap().status, "queued");
    let done_a = finish(&e, &a).await;
    let done_b = finish(&e, &b).await;
    assert!(done_b.completed_at >= done_a.completed_at);
    e.close().await.unwrap();
}
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn bandwidth_limit_and_removing_limit() {
    let f = Fixture::new().await;
    let e = f.engine();
    let mut settings = e.snapshot().settings;
    settings.bandwidth_k_b = 128;
    settings.connections = 1;
    e.settings(settings.clone()).unwrap();
    let start = Instant::now();
    finish(&e, &e.add(f.input("/small")).unwrap()).await;
    assert!(start.elapsed() >= Duration::from_millis(850));
    settings.bandwidth_k_b = 1;
    e.settings(settings.clone()).unwrap();
    let id = e.add(f.input("/small")).unwrap();
    until(|| e.get(&id).unwrap().received > 0).await;
    settings.bandwidth_k_b = 0;
    e.settings(settings).unwrap();
    let start = Instant::now();
    finish(&e, &id).await;
    assert!(start.elapsed() < Duration::from_secs(3));
    e.close().await.unwrap();
}
#[tokio::test]
async fn deleting_partial_keeps_other_downloads_and_completed_files() {
    let f = Fixture::new().await;
    let e = f.engine();
    let mut input = f.input("/file");
    input.paused = true;
    let a = e.add(input.clone()).unwrap();
    let b = e.add(input).unwrap();
    let parts = f.root.path().join("out/.relay-parts");
    for id in [&a, &b] {
        fs::create_dir_all(parts.join(id)).unwrap();
        fs::write(parts.join(id).join("0.part"), b"partial").unwrap();
    }
    fs::write(f.root.path().join("out/keep.zip"), b"complete").unwrap();
    e.action(&a, "remove").await.unwrap();
    assert!(!parts.join(&a).exists());
    assert!(parts.join(&b).exists());
    assert!(f.root.path().join("out/keep.zip").exists());
    e.close().await.unwrap();
}
#[tokio::test]
async fn unsafe_destinations_and_corrupt_history_rejected() {
    let f = Fixture::new().await;
    let e = f.engine();
    let mut input = f.input("/file");
    input.destination = Some("relative".into());
    assert!(e.add(input.clone()).is_err());
    let block = f.root.path().join("file");
    fs::write(&block, b"block").unwrap();
    input.destination = Some(block);
    assert!(e.add(input).is_err());
    assert!(e.snapshot().jobs.is_empty());
    e.close().await.unwrap();
    fs::write(f.root.path().join("data/state.json"), b"corrupt original").unwrap();
    assert!(Engine::open(f.root.path().join("data"), f.root.path().join("out")).is_err());
    assert_eq!(
        fs::read(f.root.path().join("data/state.json")).unwrap(),
        b"corrupt original"
    );
}
#[test]
fn names_urls_tokens_and_profile_lock() {
    assert_eq!(safe_name("../../bad:thing?.exe"), ".._.._bad_thing_.exe");
    assert_eq!(safe_name("CON.txt"), "_CON.txt");
    assert!(safe_name(&"文".repeat(180)).len() <= 180);
    for url in [
        "file:///etc/passwd",
        "javascript:alert(1)",
        "https://user:secret@example.org/file",
    ] {
        assert!(valid_url(url).is_err());
    }
    let dir = tempfile::tempdir().unwrap();
    let lock = ProfileLock::acquire(dir.path()).unwrap();
    assert!(ProfileLock::acquire(dir.path()).is_err());
    drop(lock);
    assert!(ProfileLock::acquire(dir.path()).is_ok());
    let token = profile::pairing_token(dir.path()).unwrap();
    assert_eq!(token.len(), 64);
    assert_eq!(profile::pairing_token(dir.path()).unwrap(), token);
}
#[test]
fn electron_json_settings_remain_compatible() {
    let settings: relay::model::Settings = serde_json::from_value(serde_json::json!({"downloadDir":"/tmp/downloads","connections":6,"simultaneous":2,"bandwidthKB":128,"organize":true})).unwrap();
    assert_eq!(settings.bandwidth_k_b, 128);
    assert_eq!(serde_json::to_value(settings).unwrap()["bandwidthKB"], 128);
}

#[test]
fn process_recovery_worker() {
    let Ok(root) = std::env::var("RELAY_RECOVERY_PROFILE") else {
        return;
    };
    let base = std::env::var("RELAY_RECOVERY_URL").unwrap();
    let root = PathBuf::from(root);
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let e = Engine::open(root.join("data"), root.join("out")).unwrap();
        let mut settings = e.snapshot().settings;
        settings.bandwidth_k_b = 1024;
        e.settings(settings).unwrap();
        let id = e
            .add(AddInput {
                url: base,
                ..Default::default()
            })
            .unwrap();
        until(|| e.get(&id).unwrap().received > 200000).await;
        e.persist().unwrap();
        fs::write(root.join("ready"), b"ready").unwrap();
        tokio::time::sleep(Duration::from_secs(60)).await;
    });
}
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn forced_process_termination_recovers_saved_bytes() {
    let f = Fixture::new().await;
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "process_recovery_worker", "--nocapture"])
        .env("RELAY_RECOVERY_PROFILE", f.root.path())
        .env("RELAY_RECOVERY_URL", format!("{}/file", f.base))
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let ready = tokio::time::timeout(Duration::from_secs(15), async {
        while !f.root.path().join("ready").exists() {
            if let Some(status) = child.try_wait().unwrap() {
                panic!("worker exited: {status}");
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    child.kill().unwrap();
    child.wait().unwrap();
    ready.unwrap();
    let e = f.engine();
    let j = e.snapshot().jobs[0].clone();
    assert_eq!(j.status, "paused");
    assert!(j.received > 0);
    e.action(&j.id, "resume").await.unwrap();
    integrity(&finish(&e, &j.id).await, &payload(0));
    e.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn different_queues_run_concurrently_up_to_the_limit() {
    let f = Fixture::new().await;
    let e = f.engine();
    let mut ids = Vec::new();
    for queue in ["one", "two", "three"] {
        let mut input = f.input("/file");
        input.queue = Some(queue.into());
        ids.push(e.add(input).unwrap());
    }
    until(|| e.snapshot().jobs.iter().filter(|j| j.active()).count() == 2).await;
    assert_eq!(e.get(&ids[2]).unwrap().status, "queued");
    for id in ids {
        integrity(&finish(&e, &id).await, &payload(0));
    }
    e.close().await.unwrap();
}

#[test]
fn atomic_profile_updates_replace_previous_state_and_preserve_on_serialization_failure() {
    struct Fails;
    impl serde::Serialize for Fails {
        fn serialize<S: serde::Serializer>(&self, _serializer: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom("simulated serialization failure"))
        }
    }
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("state.json");
    profile::atomic_json(&path, &serde_json::json!({"version":1})).unwrap();
    profile::atomic_json(&path, &serde_json::json!({"version":2})).unwrap();
    assert!(profile::atomic_json(&path, &Fails).is_err());
    let value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(value["version"], 2);
}

#[tokio::test]
async fn closed_engine_cannot_overwrite_a_reopened_profile() {
    let f = Fixture::new().await;
    let first = f.engine();
    tokio::time::sleep(Duration::from_millis(450)).await;
    first.close().await.unwrap();
    let second = f.engine();
    let mut input = f.input("/file");
    input.paused = true;
    let id = second.add(input).unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let saved: relay::model::Snapshot =
        serde_json::from_slice(&fs::read(f.root.path().join("data/state.json")).unwrap()).unwrap();
    assert_eq!(saved.jobs[0].id, id);
    second.close().await.unwrap();
}
