use crate::{model::*, profile::atomic_json};
use anyhow::{Context, Result, bail, ensure};
use chrono::Utc;
use reqwest::{Client, Response, header};
use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::{Notify, broadcast};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub fn valid_url(input: &str) -> Result<url::Url> {
    let u = url::Url::parse(input)?;
    ensure!(
        matches!(u.scheme(), "http" | "https")
            && u.host_str().is_some()
            && u.username().is_empty()
            && u.password().is_none(),
        "Use an HTTP or HTTPS URL without embedded credentials."
    );
    Ok(u)
}
pub fn safe_name(input: &str) -> String {
    let mut name = String::new();
    for c in input.trim_end_matches(['.', ' ']).chars() {
        let c = if c.is_control() || "<>:\"/\\|?*".contains(c) {
            '_'
        } else {
            c
        };
        if name.len() + c.len_utf8() > 180 {
            break;
        }
        name.push(c);
    }
    let stem = name.split('.').next().unwrap_or("").to_ascii_lowercase();
    if name.is_empty() {
        return "_download".into();
    }
    if ["con", "prn", "aux", "nul"].contains(&stem.as_str())
        || (stem.len() == 4
            && (stem.starts_with("com") || stem.starts_with("lpt"))
            && stem.as_bytes()[3].is_ascii_digit())
    {
        name.insert(0, '_');
    }
    name
}
pub fn category(name: &str) -> &'static str {
    let ext = Path::new(name)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    for (kind, extensions) in [
        ("Videos", "mp4 webm mkv mov avi m4v"),
        ("Music", "mp3 wav flac aac m4a ogg"),
        ("Archives", "zip rar 7z gz tar xz iso"),
        ("Documents", "pdf doc docx xls xlsx csv txt epub pptx"),
        ("Images", "png jpg jpeg gif svg webp avif"),
        ("Programs", "exe msi msix dmg deb rpm appimage apk"),
    ] {
        if extensions.split(' ').any(|e| e == ext) {
            return kind;
        }
    }
    "Other"
}
fn parts_dir(data: &Path, j: &Job, legacy: bool) -> Result<PathBuf> {
    ensure!(
        Uuid::parse_str(&j.id).is_ok() && !j.id.contains(['/', '\\']),
        "Invalid download ID."
    );
    Ok(if legacy {
        data.join("parts")
    } else {
        j.destination.join(".relay-parts")
    }
    .join(&j.id))
}
fn remove_parts(data: &Path, j: &Job, legacy: bool) -> Result<()> {
    let dir = parts_dir(data, j, legacy)?;
    match fs::remove_dir_all(&dir) {
        Ok(()) => (),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(e) => return Err(e.into()),
    }
    if let Some(parent) = dir.parent() {
        let _ = fs::remove_dir(parent);
    }
    Ok(())
}
fn recount(data: &Path, j: &mut Job) -> Result<()> {
    if j.status == "completed" {
        return Ok(());
    }
    let dir = parts_dir(data, j, !j.parts_in_destination)?;
    for p in &mut j.parts {
        p.done = fs::metadata(dir.join(format!("{}.part", p.start)))
            .map(|m| m.len())
            .unwrap_or(0);
        if p.length().is_some_and(|n| p.done > n) {
            p.done = 0;
        }
    }
    j.received = j.parts.iter().map(|p| p.done).sum();
    j.speed = 0.0;
    j.connections = 0;
    Ok(())
}
struct Throttle {
    next: Instant,
    epoch: u64,
}
struct Inner {
    data: PathBuf,
    state: Mutex<Snapshot>,
    active: Mutex<HashMap<String, CancellationToken>>,
    closing: AtomicBool,
    client: Client,
    throttle: Mutex<Throttle>,
    finished: Notify,
    control: tokio::sync::Mutex<()>,
    scheduler: Mutex<Option<tokio::task::JoinHandle<()>>>,
    events: broadcast::Sender<Job>,
    error: Mutex<Option<String>>,
}
#[derive(Clone)]
pub struct Engine(Arc<Inner>);
impl Engine {
    pub fn open(data: PathBuf, downloads: PathBuf) -> Result<Self> {
        fs::create_dir_all(&data)?;
        let db = data.join("state.json");
        let mut state = if db.exists() {
            serde_json::from_slice::<Snapshot>(&fs::read(&db)?)
                .context("Could not read download history; preserve state.json before recovery")?
        } else {
            Snapshot {
                settings: Settings {
                    download_dir: downloads,
                    ..Default::default()
                },
                jobs: vec![],
            }
        };
        validate_settings(&state.settings)?;
        for j in &mut state.jobs {
            ensure!(
                j.destination.is_absolute(),
                "A saved download has an invalid destination."
            );
            valid_url(&j.url)?;
            j.max_connections = j.max_connections.clamp(1, 16);
            if j.active() {
                j.status = "paused".into();
            }
            recount(&data, j)?;
            // Never trust saved names as filesystem paths, including older profiles.
            j.name = safe_name(&j.name);
            j.category = category(&j.name).into();
        }
        let client = Client::builder()
            .user_agent(concat!("Relay/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(25))
            .read_timeout(Duration::from_secs(25))
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.previous().len() > 8 {
                    attempt.error("Too many redirects")
                } else if valid_url(attempt.url().as_str()).is_err() {
                    attempt.error("Unsafe redirect URL")
                } else {
                    attempt.follow()
                }
            }))
            .build()?;
        let (events, _) = broadcast::channel(64);
        let engine = Self(Arc::new(Inner {
            data,
            state: Mutex::new(state),
            active: Mutex::new(HashMap::new()),
            closing: AtomicBool::new(false),
            client,
            throttle: Mutex::new(Throttle {
                next: Instant::now(),
                epoch: 0,
            }),
            finished: Notify::new(),
            control: tokio::sync::Mutex::new(()),
            scheduler: Mutex::new(None),
            events,
            error: Mutex::new(None),
        }));
        let e = engine.clone();
        let scheduler = tokio::spawn(async move {
            let mut timer = tokio::time::interval(Duration::from_millis(250));
            let mut ticks = 0;
            while !e.0.closing.load(Ordering::SeqCst) {
                timer.tick().await;
                if e.0.closing.load(Ordering::SeqCst) {
                    break;
                }
                e.pump();
                ticks += 1;
                if ticks % 3 == 0
                    && let Err(error) = e.persist()
                {
                    *e.0.error.lock().unwrap() =
                        Some(format!("Could not save download history: {error}"));
                    e.0.closing.store(true, Ordering::SeqCst);
                    for c in e.0.active.lock().unwrap().values() {
                        c.cancel();
                    }
                }
            }
        });
        *engine.0.scheduler.lock().unwrap() = Some(scheduler);
        Ok(engine)
    }
    pub fn snapshot(&self) -> Snapshot {
        self.0.state.lock().unwrap().clone()
    }
    pub fn storage_error(&self) -> Option<String> {
        self.0.error.lock().unwrap().clone()
    }
    pub fn subscribe(&self) -> broadcast::Receiver<Job> {
        self.0.events.subscribe()
    }
    pub fn get(&self, id: &str) -> Result<Job> {
        self.snapshot()
            .jobs
            .into_iter()
            .find(|j| j.id == id)
            .context("Download not found.")
    }
    pub fn persist(&self) -> Result<()> {
        atomic_json(
            &self.0.data.join("state.json"),
            &*self.0.state.lock().unwrap(),
        )
    }
    fn update(&self, id: &str, f: impl FnOnce(&mut Job)) {
        if let Some(j) = self
            .0
            .state
            .lock()
            .unwrap()
            .jobs
            .iter_mut()
            .find(|j| j.id == id)
        {
            f(j);
        }
    }
    pub fn add(&self, input: AddInput) -> Result<String> {
        ensure!(
            !self.0.closing.load(Ordering::SeqCst),
            "The engine is closing or cannot save its history."
        );
        let url = valid_url(&input.url)?;
        let mut state = self.0.state.lock().unwrap();
        ensure!(
            state
                .jobs
                .iter()
                .filter(|j| j.status != "completed")
                .count()
                < 500,
            "The queue is full (500 unfinished downloads)."
        );
        let destination = input
            .destination
            .unwrap_or_else(|| state.settings.download_dir.clone());
        ensure!(
            destination.is_absolute(),
            "Choose an absolute download directory."
        );
        fs::create_dir_all(&destination)?;
        let check = tempfile::NamedTempFile::new_in(&destination)
            .context("The download directory is not writable")?;
        drop(check);
        let fallback = percent_encoding::percent_decode_str(
            url.path_segments()
                .and_then(|mut p| p.next_back())
                .unwrap_or("download"),
        )
        .decode_utf8_lossy()
        .into_owned();
        let name = input.name.filter(|n| !n.is_empty());
        let job = Job {
            id: Uuid::new_v4().to_string(),
            url: url.into(),
            name: safe_name(name.as_deref().unwrap_or(&fallback)),
            custom_name: name.is_some(),
            queue: input
                .queue
                .filter(|q| !q.trim().is_empty())
                .unwrap_or_else(|| "Main queue".into())
                .chars()
                .take(80)
                .collect(),
            scheduled_at: input.scheduled_at,
            max_connections: input
                .connections
                .unwrap_or(state.settings.connections)
                .clamp(1, 16),
            status: if input.paused { "paused" } else { "queued" }.into(),
            size: None,
            received: 0,
            speed: 0.0,
            parts: vec![],
            created_at: Utc::now(),
            error: None,
            category: "Other".into(),
            destination,
            parts_in_destination: true,
            organize: state.settings.organize,
            ranges: false,
            validator: None,
            file_path: None,
            completed_at: None,
            last_url_update_at: None,
            connections: 0,
        };
        let id = job.id.clone();
        state.jobs.push(job);
        if let Err(e) = atomic_json(&self.0.data.join("state.json"), &*state) {
            state.jobs.pop();
            return Err(e);
        }
        Ok(id)
    }
    pub fn settings(&self, settings: Settings) -> Result<()> {
        validate_settings(&settings)?;
        fs::create_dir_all(&settings.download_dir)?;
        let check = tempfile::NamedTempFile::new_in(&settings.download_dir)?;
        drop(check);
        let mut state = self.0.state.lock().unwrap();
        let previous = std::mem::replace(&mut state.settings, settings);
        if let Err(e) = atomic_json(&self.0.data.join("state.json"), &*state) {
            state.settings = previous;
            return Err(e);
        }
        let mut throttle = self.0.throttle.lock().unwrap();
        throttle.epoch += 1;
        throttle.next = Instant::now();
        Ok(())
    }
    pub async fn action(&self, id: &str, action: &str) -> Result<()> {
        let _control = self.0.control.lock().await;
        self.action_locked(id, action).await
    }
    async fn action_locked(&self, id: &str, action: &str) -> Result<()> {
        self.get(id)?;
        match action {
            "pause" | "remove" => {
                {
                    let mut state = self.0.state.lock().unwrap();
                    let active = self.0.active.lock().unwrap();
                    let job = state
                        .jobs
                        .iter_mut()
                        .find(|j| j.id == id)
                        .context("Download not found")?;
                    if job.status != "completed" {
                        job.status = "paused".into();
                        job.speed = 0.0;
                    }
                    if let Some(c) = active.get(id) {
                        c.cancel();
                    }
                }
                loop {
                    let notified = self.0.finished.notified();
                    tokio::pin!(notified);
                    notified.as_mut().enable();
                    if !self.0.active.lock().unwrap().contains_key(id) {
                        break;
                    }
                    notified.await;
                }
                if action == "remove" {
                    let job = self.get(id)?;
                    remove_parts(&self.0.data, &job, false)?;
                    remove_parts(&self.0.data, &job, true)?;
                    self.0.state.lock().unwrap().jobs.retain(|j| j.id != id);
                }
            }
            "resume" => self.update(id, |j| {
                if matches!(j.status.as_str(), "paused" | "error") {
                    j.status = "queued".into();
                    j.error = None;
                    j.scheduled_at = None;
                }
            }),
            "up" => {
                let mut state = self.0.state.lock().unwrap();
                if let Some(i) = state.jobs.iter().position(|j| j.id == id)
                    && i > 0
                {
                    state.jobs.swap(i, i - 1);
                }
            }
            _ => bail!("Unknown action."),
        }
        self.persist()
    }
    /// Validate a new source without discarding any saved segments.
    pub async fn replace_url(&self, id: &str, input: &str, resume: bool) -> Result<Job> {
        let url = valid_url(input.trim())?.to_string();
        // Serialize with pause/remove/resume and shutdown for the whole transaction.
        let _control = self.0.control.lock().await;
        ensure!(!self.0.closing.load(Ordering::SeqCst), "Relay is closing.");
        ensure!(
            self.get(id)?.status != "completed",
            "This download is already complete."
        );
        self.action_locked(id, "pause").await?;
        let mut previous = self.get(id)?;
        ensure!(
            previous.status != "completed",
            "This download finished before its link could be updated."
        );
        recount(&self.0.data, &mut previous)?;
        self.update(id, |j| *j = previous.clone());
        self.persist()?;
        let (size, ranges, validator, _) = tokio::time::timeout(
            Duration::from_secs(30),
            self.probe(&url, &CancellationToken::new()),
        )
        .await
        .context("Replacement link check timed out. Saved progress has been kept.")??;
        if previous.received > 0 {
            ensure!(
                ranges,
                "The replacement link does not support byte ranges. Saved progress has been kept."
            );
            ensure!(
                previous.size.is_some() && size == previous.size,
                "The replacement file has a different or unknown size. Saved progress has been kept."
            );
            ensure!(
                previous.validator.is_some() && validator == previous.validator,
                "The replacement file identifier does not match, or cannot be verified. Saved progress has been kept; use a link to the same file."
            );
            let checkpoint = previous
                .destination
                .join(".relay-recovery")
                .join(&previous.id);
            fs::create_dir_all(&checkpoint)?;
            atomic_json(
                &checkpoint.join(format!("before-url-update-{}.json", Uuid::new_v4())),
                &previous,
            )?;
        }
        let mut state = self.0.state.lock().unwrap();
        let index = state
            .jobs
            .iter()
            .position(|j| j.id == id)
            .context("Download not found")?;
        let j = &mut state.jobs[index];
        j.url = url;
        j.status = if resume { "queued" } else { "paused" }.into();
        j.error = None;
        j.custom_name = true;
        j.scheduled_at = None;
        j.last_url_update_at = Some(Utc::now());
        if let Err(error) = atomic_json(&self.0.data.join("state.json"), &*state) {
            state.jobs[index] = previous;
            return Err(error);
        }
        Ok(state.jobs[index].clone())
    }
    fn pump(&self) {
        if self.0.closing.load(Ordering::SeqCst) {
            return;
        }
        let mut state = self.0.state.lock().unwrap();
        let mut active = self.0.active.lock().unwrap();
        let limit = state.settings.simultaneous;
        let mut queues: Vec<String> = state
            .jobs
            .iter()
            .filter(|j| active.contains_key(&j.id))
            .map(|j| j.queue.clone())
            .collect();
        for j in &mut state.jobs {
            if active.len() >= limit {
                break;
            }
            if j.status != "queued"
                || active.contains_key(&j.id)
                || queues.contains(&j.queue)
                || j.scheduled_at.is_some_and(|at| at > Utc::now())
            {
                continue;
            }
            j.status = "probing".into();
            j.error = None;
            queues.push(j.queue.clone());
            let cancel = CancellationToken::new();
            active.insert(j.id.clone(), cancel.clone());
            let e = self.clone();
            let id = j.id.clone();
            tokio::spawn(async move {
                let result = e.run(&id, &cancel).await;
                // Keep the state and active map lock order consistent with the scheduler.
                {
                    let mut state = e.0.state.lock().unwrap();
                    if let Some(job) = state.jobs.iter_mut().find(|j| j.id == id) {
                        job.speed = 0.0;
                        job.connections = 0;
                        if let Err(error) = result
                            && job.status != "paused"
                        {
                            if job.status != "completed" {
                                job.status = "error".into();
                            }
                            job.error = Some(error.to_string());
                        }
                    }
                }
                if let Err(error) = e.persist() {
                    *e.0.error.lock().unwrap() = Some(error.to_string());
                }
                // Removal is the completion barrier: close() must wait for the
                // final history write before another engine opens this profile.
                e.0.active.lock().unwrap().remove(&id);
                e.0.finished.notify_waiters();
            });
        }
    }
    pub async fn close(&self) -> Result<()> {
        let _control = self.0.control.lock().await;
        self.0.closing.store(true, Ordering::SeqCst);
        let scheduler = self.0.scheduler.lock().unwrap().take();
        if let Some(scheduler) = scheduler {
            scheduler.abort();
            let _ = scheduler.await;
        }
        {
            let mut state = self.0.state.lock().unwrap();
            let active = self.0.active.lock().unwrap();
            for j in &mut state.jobs {
                if let Some(c) = active.get(&j.id) {
                    if j.active() {
                        j.status = "paused".into();
                    }
                    c.cancel();
                }
            }
        }
        loop {
            let wait = self.0.finished.notified();
            tokio::pin!(wait);
            wait.as_mut().enable();
            if self.0.active.lock().unwrap().is_empty() {
                break;
            }
            wait.await;
        }
        self.persist()
    }
    async fn request(
        &self,
        url: &str,
        range: Option<String>,
        validator: Option<&str>,
        cancel: &CancellationToken,
    ) -> Result<Response> {
        valid_url(url)?;
        let mut req = self
            .0
            .client
            .get(url)
            .header(header::ACCEPT_ENCODING, "identity");
        if let Some(range) = range {
            req = req.header(header::RANGE, range);
        }
        if let Some(v) = validator {
            req = req.header(header::IF_RANGE, v);
        }
        tokio::select! { _ = cancel.cancelled() => bail!("Paused."), response = req.send() => Ok(response?) }
    }
    fn migrate(&self, id: &str) -> Result<()> {
        let j = self.get(id)?;
        if j.parts_in_destination {
            return remove_parts(&self.0.data, &j, true);
        }
        let source = parts_dir(&self.0.data, &j, true)?;
        let target = parts_dir(&self.0.data, &j, false)?;
        remove_parts(&self.0.data, &j, false)?;
        fs::create_dir_all(&target)?;
        for p in &j.parts {
            let name = format!("{}.part", p.start);
            if source.join(&name).exists() {
                fs::copy(source.join(&name), target.join(&name))?;
                OpenOptions::new()
                    .write(true)
                    .open(target.join(&name))?
                    .sync_all()?;
            }
        }
        self.update(id, |j| j.parts_in_destination = true);
        if let Err(e) = self.persist() {
            self.update(id, |j| j.parts_in_destination = false);
            return Err(e);
        }
        remove_parts(&self.0.data, &j, true)?;
        Ok(())
    }
    async fn probe(
        &self,
        url: &str,
        cancel: &CancellationToken,
    ) -> Result<(Option<u64>, bool, Option<String>, Option<String>)> {
        let response = self
            .request(url, Some("bytes=0-0".into()), None, cancel)
            .await?;
        let status = response.status().as_u16();
        let h = response.headers();
        let content_range = text(h, "content-range");
        let range_size = content_range
            .as_deref()
            .and_then(|s| s.strip_prefix("bytes 0-0/"))
            .and_then(|s| s.parse::<u64>().ok());
        let empty = status == 416 && content_range.as_deref() == Some("bytes */0");
        ensure!(
            empty || matches!(status, 200 | 206),
            "{}",
            http_error(status)
        );
        let size = if empty {
            Some(0)
        } else if status == 206 {
            Some(range_size.context("Invalid range probe response")?)
        } else {
            text(h, "content-length").map(|v| v.parse()).transpose()?
        };
        ensure!(
            size.is_none_or(|n| n <= i64::MAX as u64),
            "Invalid file size from server."
        );
        let ranges = status == 206
            && range_size.is_some()
            && text(h, "content-encoding").is_none_or(|s| s == "identity");
        let validator = validator(h);
        Ok((size, ranges, validator, text(h, "content-disposition")))
    }
    async fn run(&self, id: &str, cancel: &CancellationToken) -> Result<()> {
        self.migrate(id)?;
        let mut j = self.get(id)?;
        let (size, ranges, validator, disposition) = self.probe(&j.url, cancel).await?;
        if !j.custom_name
            && let Some(disposition) = disposition
            && let Some(name) = disposition_name(&disposition)
        {
            j.name = safe_name(&name);
        }
        ensure!(!cancel.is_cancelled(), "Paused.");
        let keep = !j.parts.is_empty()
            && ranges
            && validator.is_some()
            && validator == j.validator
            && size == j.size;
        recount(&self.0.data, &mut j)?;
        ensure!(
            j.received == 0 || keep,
            "The server cannot safely resume this file: its size, identifier, or range support changed. Saved progress has been kept. Use Update link with a link to the same file."
        );
        if !keep {
            remove_parts(&self.0.data, &j, false)?;
            j.parts.clear();
            j.received = 0;
        }
        let dir = parts_dir(&self.0.data, &j, false)?;
        fs::create_dir_all(&dir)?;
        j.size = size;
        j.ranges = ranges;
        j.validator = validator;
        j.category = category(&j.name).into();
        if j.parts.is_empty() {
            if ranges && size.unwrap_or(0) > 0 {
                let size = size.unwrap();
                let chunk = (size.div_ceil((j.max_connections * 8) as u64)).max(256 * 1024);
                let mut start = 0;
                while start < size {
                    j.parts.push(Part {
                        start,
                        end: Some((size - 1).min(start + chunk - 1) as i64),
                        done: 0,
                    });
                    start += chunk;
                }
            } else {
                j.parts.push(Part {
                    start: 0,
                    end: size.map(|n| n as i64 - 1),
                    done: 0,
                });
            }
        }
        recount(&self.0.data, &mut j)?;
        j.status = "downloading".into();
        self.update(id, |current| {
            // A concurrent pause must win over the probe.
            if current.status != "paused" {
                *current = j.clone();
            }
        });
        ensure!(!cancel.is_cancelled(), "Paused.");
        self.persist()?;
        let pending: Vec<usize> = j
            .parts
            .iter()
            .enumerate()
            .filter(|(_, p)| p.length().is_none_or(|len| p.done < len) || size == Some(0))
            .map(|(i, _)| i)
            .collect();
        let pending = Arc::new(Mutex::new(pending.into_iter()));
        let workers = if ranges {
            j.max_connections.min(j.parts.len())
        } else {
            1
        };
        self.update(id, |j| j.connections = workers);
        let mut tasks = tokio::task::JoinSet::new();
        for _ in 0..workers {
            let e = self.clone();
            let id = id.to_string();
            let pending = pending.clone();
            let cancel = cancel.clone();
            tasks.spawn(async move {
                loop {
                    let index = pending.lock().unwrap().next(); let Some(index) = index else { return Ok::<(), anyhow::Error>(()); };
                    let mut last = None;
                    for attempt in 0..3 {
                        if cancel.is_cancelled() { bail!("Paused."); }
                        match e.transfer(&id, index, &cancel).await {
                            Ok(()) => { last = None; break; }
                            Err(error) => {
                                let fatal = error.downcast_ref::<ProtocolError>().is_some(); last = Some(error);
                                if fatal || attempt == 2 { break; }
                                tokio::select! { _ = cancel.cancelled() => bail!("Paused."), _ = tokio::time::sleep(Duration::from_millis(350 * (attempt + 1))) => () }
                            }
                        }
                    }
                    if let Some(error) = last { return Err(error); }
                }
            });
        }
        let mut failure = None;
        while let Some(result) = tasks.join_next().await {
            match result {
                Ok(Ok(())) => (),
                Ok(Err(e)) => {
                    if failure.is_none() {
                        failure = Some(e);
                    }
                    cancel.cancel();
                }
                Err(e) => {
                    if failure.is_none() {
                        failure = Some(e.into());
                    }
                    cancel.cancel();
                }
            }
        }
        if let Some(error) = failure {
            return Err(error);
        }
        ensure!(!cancel.is_cancelled(), "Paused.");
        self.update(id, |j| {
            if j.status != "paused" {
                j.status = "assembling".into();
            }
            j.connections = 0;
        });
        self.persist()?;
        self.assemble(id, cancel)?;
        // Publication is the commit point; once the final file exists it must be recorded as completed, even if pause arrives now.
        self.update(id, |j| {
            j.status = "completed".into();
            j.completed_at = Some(Utc::now());
            j.received = j.size.unwrap_or(j.received);
        });
        self.persist()?;
        let completed = self.get(id)?;
        remove_parts(&self.0.data, &completed, false)?;
        let _ = self.0.events.send(completed);
        Ok(())
    }
    async fn transfer(&self, id: &str, index: usize, cancel: &CancellationToken) -> Result<()> {
        let j = self.get(id)?;
        let mut part = j.parts[index].clone();
        if !j.ranges {
            self.update(id, |j| {
                j.received = j.received.saturating_sub(part.done);
                j.parts[index].done = 0;
            });
            part.done = 0;
        }
        let start = part.start + part.done;
        let range = if j.ranges {
            Some(format!("bytes={start}-{}", part.end.unwrap()))
        } else {
            None
        };
        let mut response = self
            .request(
                &j.url,
                range,
                if j.ranges {
                    j.validator.as_deref()
                } else {
                    None
                },
                cancel,
            )
            .await?;
        if !response.status().is_success() {
            return Err(ProtocolError(http_error(response.status().as_u16())).into());
        }
        if j.ranges {
            if response.status().as_u16() != 206
                || text(response.headers(), "content-range")
                    != Some(format!(
                        "bytes {start}-{}/{}",
                        part.end.unwrap(),
                        j.size.unwrap()
                    ))
            {
                return Err(ProtocolError(
                    "Server changed or ignored a range. Saved progress has been kept. Use Update link with a link to the same file.".into(),
                )
                .into());
            }
            if let (Some(old), Some(new)) = (&j.validator, validator(response.headers()))
                && *old != new
            {
                return Err(ProtocolError(
                    "The remote file changed. Saved progress has been kept. Use Update link with a link to the same file.".into(),
                )
                .into());
            }
        } else if response.status().as_u16() != 200 {
            return Err(ProtocolError(format!(
                "Server returned HTTP {}.",
                response.status().as_u16()
            ))
            .into());
        }
        if text(response.headers(), "content-encoding").is_some_and(|v| v != "identity") {
            return Err(ProtocolError(
                "Server sent compressed transfer data despite requesting identity encoding.".into(),
            )
            .into());
        }
        let file = parts_dir(&self.0.data, &j, false)?.join(format!("{}.part", part.start));
        let mut output = OpenOptions::new()
            .create(true)
            .write(true)
            .append(part.done > 0)
            .truncate(part.done == 0)
            .open(file)?;
        let mut meter = Instant::now();
        let mut last_bytes = self.get(id)?.received;
        let result: Result<()> = async {
            loop {
                let chunk = tokio::select! { _ = cancel.cancelled() => bail!("Paused."), chunk = response.chunk() => chunk? };
                let Some(chunk) = chunk else { break; };
                if part.length().is_some_and(|n| part.done + chunk.len() as u64 > n) { return Err(ProtocolError("Server sent more data than expected.".into()).into()); }
                output.write_all(&chunk)?; part.done += chunk.len() as u64;
                self.update(id, |j| { j.received += chunk.len() as u64; j.parts[index].done = part.done;
                    if meter.elapsed() >= Duration::from_millis(750) { j.speed = j.received.saturating_sub(last_bytes) as f64 / meter.elapsed().as_secs_f64(); last_bytes = j.received; meter = Instant::now(); }
                });
                self.throttle(chunk.len(), cancel).await?;
            }
            ensure!(part.length().is_none_or(|n| part.done == n), "Connection closed before the segment finished.");
            if j.size.is_none() { self.update(id, |j| { j.size = Some(part.done); j.parts[index].end = Some(part.done as i64 - 1); }); }
            Ok(())
        }.await;
        output.sync_all()?;
        result
    }
    async fn throttle(&self, bytes: usize, cancel: &CancellationToken) -> Result<()> {
        let (until, epoch) = {
            // Reserve under the same lock order as settings(), so removing a cap
            // invalidates every reservation made using the previous rate.
            let state = self.0.state.lock().unwrap();
            let rate = state.settings.bandwidth_k_b * 1024;
            if rate == 0 {
                return Ok(());
            }
            let mut t = self.0.throttle.lock().unwrap();
            t.next =
                t.next.max(Instant::now()) + Duration::from_secs_f64(bytes as f64 / rate as f64);
            (t.next, t.epoch)
        };
        while Instant::now() < until {
            if self.0.throttle.lock().unwrap().epoch != epoch {
                break;
            }
            tokio::select! { _ = cancel.cancelled() => bail!("Paused."), _ = tokio::time::sleep(until.saturating_duration_since(Instant::now()).min(Duration::from_millis(100))) => () }
        }
        Ok(())
    }
    fn assemble(&self, id: &str, cancel: &CancellationToken) -> Result<()> {
        let j = self.get(id)?;
        let folder = if j.organize {
            j.destination.join(category(&j.name))
        } else {
            j.destination.clone()
        };
        fs::create_dir_all(&folder)?;
        let mut staging = tempfile::NamedTempFile::new_in(&folder)?;
        let dir = parts_dir(&self.0.data, &j, false)?;
        let mut buffer = vec![0_u8; 64 * 1024];
        for part in &j.parts {
            let mut input = fs::File::open(dir.join(format!("{}.part", part.start)))?;
            loop {
                ensure!(!cancel.is_cancelled(), "Paused.");
                let n = input.read(&mut buffer)?;
                if n == 0 {
                    break;
                }
                staging.write_all(&buffer[..n])?;
            }
        }
        staging.as_file().sync_all()?;
        let name = safe_name(&j.name);
        let path = Path::new(&name);
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("download");
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .map(|e| format!(".{e}"))
            .unwrap_or_default();
        for n in 0_u64.. {
            ensure!(!cancel.is_cancelled(), "Paused.");
            let target = folder.join(if n == 0 {
                name.clone()
            } else {
                format!("{stem} ({n}){ext}")
            });
            match fs::hard_link(staging.path(), &target) {
                Ok(()) => {
                    self.update(id, |j| j.file_path = Some(target));
                    return Ok(());
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e)
                    if e.kind() == std::io::ErrorKind::PermissionDenied
                        || e.kind() == std::io::ErrorKind::Unsupported =>
                {
                    match OpenOptions::new()
                        .create_new(true)
                        .write(true)
                        .open(&target)
                    {
                        Ok(mut file) => {
                            let copy = (|| -> Result<()> {
                                let mut input = fs::File::open(staging.path())?;
                                std::io::copy(&mut input, &mut file)?;
                                file.sync_all()?;
                                Ok(())
                            })();
                            drop(file);
                            if let Err(error) = copy {
                                let _ = fs::remove_file(&target);
                                return Err(error);
                            }
                            self.update(id, |j| j.file_path = Some(target));
                            return Ok(());
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                        Err(e) => return Err(e.into()),
                    }
                }
                Err(e) => return Err(e.into()),
            }
        }
        unreachable!()
    }
}
fn validate_settings(s: &Settings) -> Result<()> {
    ensure!(
        s.download_dir.is_absolute(),
        "Choose an absolute download directory."
    );
    ensure!(
        (1..=16).contains(&s.connections)
            && (1..=8).contains(&s.simultaneous)
            && s.bandwidth_k_b <= 1_048_576,
        "Invalid connection, simultaneous download, or bandwidth setting."
    );
    Ok(())
}
fn text(h: &header::HeaderMap, key: &str) -> Option<String> {
    h.get(key).and_then(|s| s.to_str().ok()).map(str::to_string)
}
fn validator(h: &header::HeaderMap) -> Option<String> {
    text(h, "etag")
        .filter(|s| !s.starts_with("W/"))
        .or_else(|| text(h, "last-modified"))
}
fn disposition_name(value: &str) -> Option<String> {
    if let Some(encoded) = value.split(';').map(str::trim).find_map(|p| {
        p.strip_prefix("filename*=UTF-8''")
            .or_else(|| p.strip_prefix("filename*=utf-8''"))
    }) {
        return percent_encoding::percent_decode_str(encoded)
            .decode_utf8()
            .ok()
            .map(|s| s.into_owned());
    }
    value
        .split(';')
        .map(str::trim)
        .find_map(|p| p.strip_prefix("filename="))
        .map(|s| s.trim_matches('"').to_string())
}
#[derive(Debug)]
struct ProtocolError(String);
impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for ProtocolError {}

fn http_error(status: u16) -> String {
    if matches!(status, 401 | 403 | 404 | 410) {
        format!(
            "Server returned HTTP {status}. The link may have expired. Use Update link to continue; saved progress has been kept."
        )
    } else {
        format!("Server returned HTTP {status}.")
    }
}
