use crate::assets;
use anyhow::{Context, Result};
use chrono::{Local, NaiveDateTime, TimeZone};
use eframe::egui::{self, Color32, RichText};
use relay::{
    bridge::FolderRequest,
    engine::Engine,
    model::{AddInput, Job, Settings},
};
use std::{collections::VecDeque, path::PathBuf, time::Duration};
use tokio::sync::{broadcast, mpsc};

enum Message {
    Error(String),
    Folder(PathBuf, bool),
    BrowserDialogDone,
}
#[derive(Default)]
struct DownloadForm {
    url: String,
    name: String,
    destination: String,
    queue: String,
    connections: usize,
    paused: bool,
    scheduled: String,
}
pub struct Desktop {
    engine: Engine,
    token: String,
    extension_dir: PathBuf,
    bridge_status: String,
    folders: mpsc::UnboundedReceiver<FolderRequest>,
    pending_folders: VecDeque<FolderRequest>,
    folder_busy: bool,
    messages: mpsc::UnboundedReceiver<Message>,
    send: mpsc::UnboundedSender<Message>,
    completions: broadcast::Receiver<Job>,
    view: String,
    search: String,
    sort: usize,
    queue_filter: String,
    message: Option<String>,
    show_add: bool,
    form: DownloadForm,
    preferences: Settings,
    smoke: Option<PathBuf>,
    frames: usize,
}
impl Desktop {
    pub fn new(
        engine: Engine,
        token: String,
        extension_dir: PathBuf,
        bridge_status: String,
        folders: mpsc::UnboundedReceiver<FolderRequest>,
        smoke: Option<PathBuf>,
    ) -> Self {
        let (send, messages) = mpsc::unbounded_channel();
        let preferences = engine.snapshot().settings;
        let completions = engine.subscribe();
        Self {
            engine,
            token,
            extension_dir,
            bridge_status,
            folders,
            pending_folders: VecDeque::new(),
            folder_busy: false,
            messages,
            send,
            completions,
            view: "All downloads".into(),
            search: String::new(),
            sort: 0,
            queue_filter: String::new(),
            message: None,
            show_add: false,
            form: DownloadForm::default(),
            preferences,
            smoke,
            frames: 0,
        }
    }
    fn action(&self, id: String, action: &'static str) {
        let engine = self.engine.clone();
        let send = self.send.clone();
        tokio::spawn(async move {
            if let Err(error) = engine.action(&id, action).await {
                let _ = send.send(Message::Error(error.to_string()));
            }
        });
    }
    fn new_download(&mut self) {
        self.form = DownloadForm {
            destination: self
                .engine
                .snapshot()
                .settings
                .download_dir
                .display()
                .to_string(),
            queue: "Main queue".into(),
            connections: self.engine.snapshot().settings.connections,
            ..Default::default()
        };
        self.show_add = true;
    }
    fn pick_folder(&self, for_download: bool, current: String) {
        let send = self.send.clone();
        tokio::spawn(async move {
            if let Some(folder) = rfd::AsyncFileDialog::new()
                .set_title("Choose a download directory")
                .set_directory(current)
                .pick_folder()
                .await
            {
                let _ = send.send(Message::Folder(folder.path().to_owned(), for_download));
            }
        });
    }
    fn open(&mut self, path: PathBuf) {
        if let Err(e) = open::that(path) {
            self.message = Some(e.to_string());
        }
    }
    fn collect_events(&mut self, ctx: &egui::Context) {
        while let Ok(message) = self.messages.try_recv() {
            match message {
                Message::Error(error) => self.message = Some(error),
                Message::Folder(path, true) => self.form.destination = path.display().to_string(),
                Message::Folder(path, false) => self.preferences.download_dir = path,
                Message::BrowserDialogDone => self.folder_busy = false,
            }
        }
        while let Ok(request) = self.folders.try_recv() {
            self.pending_folders.push_back(request);
        }
        if !self.folder_busy
            && let Some(request) = self.pending_folders.pop_front()
            && !request.reply.is_closed()
        {
            self.folder_busy = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            let send = self.send.clone();
            tokio::spawn(async move {
                // Dropping the HTTP response receiver also invalidates this handoff.
                if !request.reply.is_closed() {
                    let selected = rfd::AsyncFileDialog::new()
                        .set_title(format!("Relay browser download — {}", request.url))
                        .set_directory(request.destination)
                        .pick_folder()
                        .await;
                    let _ = request
                        .reply
                        .send(selected.map(|file| file.path().to_path_buf()));
                }
                let _ = send.send(Message::BrowserDialogDone);
            });
        }
        loop {
            match self.completions.try_recv() {
                Ok(job) => {
                    std::thread::spawn(move || {
                        let _ = notify_rust::Notification::new()
                            .summary("Download complete")
                            .body(&job.name)
                            .appname("Relay")
                            .show();
                    });
                }
                Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
        if let Some(error) = self.engine.storage_error() {
            self.message = Some(error);
        }
    }
    fn settings_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("Settings & browser integration");
        ui.add_space(16.0);
        egui::Grid::new("settings")
            .spacing([24.0, 16.0])
            .show(ui, |ui| {
                ui.label("Download directory");
                ui.horizontal(|ui| {
                    ui.label(self.preferences.download_dir.display().to_string());
                    if ui.button("Browse…").clicked() {
                        self.pick_folder(
                            false,
                            self.preferences.download_dir.display().to_string(),
                        );
                    }
                });
                ui.end_row();
                ui.label("Connections per file");
                ui.add(egui::Slider::new(&mut self.preferences.connections, 1..=16));
                ui.end_row();
                ui.label("Simultaneous downloads");
                ui.add(egui::Slider::new(&mut self.preferences.simultaneous, 1..=8));
                ui.end_row();
                ui.label("Shared bandwidth cap (KB/s)");
                ui.add(
                    egui::DragValue::new(&mut self.preferences.bandwidth_k_b).range(0..=1_048_576),
                );
                ui.end_row();
                ui.label("");
                ui.label("0 means unlimited");
                ui.end_row();
                ui.label("Organize files");
                ui.checkbox(&mut self.preferences.organize, "Use category subfolders");
                ui.end_row();
            });
        ui.add_space(12.0);
        if ui.button("Save preferences").clicked() {
            self.message = Some(match self.engine.settings(self.preferences.clone()) {
                Ok(()) => "Preferences saved.".into(),
                Err(e) => e.to_string(),
            });
        }
        ui.add_space(26.0);
        ui.separator();
        ui.heading("Connect your browser");
        ui.label(&self.bridge_status);
        ui.label("1. Open the extension folder and load chromium in Chrome/Edge, or firefox/manifest.json in Firefox.");
        ui.label("2. Copy the pairing token, paste it into the extension, and click Connect.");
        ui.label("3. Browser downloads ask you to choose a destination in Relay.");
        ui.horizontal(|ui| {
            if ui.button("Open extension folder").clicked() {
                self.open(self.extension_dir.clone());
            }
            if ui.button("Copy pairing token").clicked() {
                ui.ctx().copy_text(self.token.clone());
                self.message = Some("Pairing token copied. Keep it private.".into());
            }
        });
        ui.add_space(12.0);
        ui.label("Direct HTTP(S) files only. DRM, HLS/DASH, and browser login cookies are not supported.");
        ui.label(format!("Relay {}", env!("CARGO_PKG_VERSION")));
    }
    fn add_ui(&mut self, ctx: &egui::Context) {
        let mut open = self.show_add;
        let mut submit = false;
        egui::Window::new("New download")
            .fade_in(false)
            .fade_out(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(550.0)
            .show(ctx, |ui| {
                egui::Grid::new("download-form")
                    .num_columns(2)
                    .spacing([16.0, 12.0])
                    .show(ui, |ui| {
                        ui.label("Direct URL");
                        ui.add(egui::TextEdit::singleline(&mut self.form.url).desired_width(400.0));
                        ui.end_row();
                        ui.label("File name (optional)");
                        ui.text_edit_singleline(&mut self.form.name);
                        ui.end_row();
                        ui.label("Download directory");
                        ui.horizontal(|ui| {
                            ui.text_edit_singleline(&mut self.form.destination);
                            if ui.button("Browse…").clicked() {
                                self.pick_folder(true, self.form.destination.clone());
                            }
                        });
                        ui.end_row();
                        ui.label("Queue");
                        ui.text_edit_singleline(&mut self.form.queue);
                        ui.end_row();
                        ui.label("Connections");
                        ui.add(egui::Slider::new(&mut self.form.connections, 1..=16));
                        ui.end_row();
                        ui.label("Schedule (local time)");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.form.scheduled)
                                .hint_text("YYYY-MM-DD HH:MM, or leave empty"),
                        );
                        ui.end_row();
                        ui.label("");
                        ui.checkbox(&mut self.form.paused, "Add paused");
                        ui.end_row();
                    });
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.button("Paste URL").clicked() {
                        match arboard::Clipboard::new().and_then(|mut c| c.get_text()) {
                            Ok(text) => self.form.url = text.trim().into(),
                            Err(e) => self.message = Some(e.to_string()),
                        }
                    }
                    submit = ui.button("Start download").clicked();
                });
            });
        self.show_add = open;
        if submit {
            let input: Result<AddInput> = (|| {
                let scheduled_at = if self.form.scheduled.trim().is_empty() {
                    None
                } else {
                    let naive =
                        NaiveDateTime::parse_from_str(self.form.scheduled.trim(), "%Y-%m-%d %H:%M")
                            .context("Use YYYY-MM-DD HH:MM for the schedule.")?;
                    Some(
                        Local
                            .from_local_datetime(&naive)
                            .single()
                            .context(
                                "That local time is invalid or ambiguous; choose another time.",
                            )?
                            .to_utc(),
                    )
                };
                Ok(AddInput {
                    url: self.form.url.trim().into(),
                    name: Some(self.form.name.trim().into()),
                    destination: Some(PathBuf::from(self.form.destination.trim())),
                    queue: Some(self.form.queue.trim().into()),
                    connections: Some(self.form.connections),
                    paused: self.form.paused,
                    scheduled_at,
                })
            })();
            match input.and_then(|input| self.engine.add(input)) {
                Ok(_) => {
                    self.show_add = false;
                    self.message = Some("Download added.".into());
                }
                Err(e) => self.message = Some(e.to_string()),
            }
        }
    }
    fn download_row(&mut self, ui: &mut egui::Ui, job: &Job) {
        egui::Frame::group(ui.style())
            .inner_margin(16.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(RichText::new(&job.name).strong().size(17.0));
                        ui.label(
                            RichText::new(format!(
                                "{} · {} · {}",
                                job.category,
                                job.queue,
                                url::Url::parse(&job.url)
                                    .ok()
                                    .and_then(|u| u.host_str().map(str::to_string))
                                    .unwrap_or_default()
                            ))
                            .weak(),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .button("Remove")
                            .on_hover_text("Remove entry and partial data; keep completed file")
                            .clicked()
                        {
                            self.action(job.id.clone(), "remove");
                        }
                        if job.status == "completed" && ui.button("Show in folder").clicked() {
                            if let Some(path) = &job.file_path
                                && path.exists()
                            {
                                self.open(path.parent().unwrap_or(path).to_path_buf());
                            } else {
                                self.message = Some(
                                    "The completed file is no longer at its saved location.".into(),
                                );
                            }
                        }
                        if matches!(job.status.as_str(), "paused" | "error")
                            && ui.button("Resume").clicked()
                        {
                            self.action(job.id.clone(), "resume");
                        }
                        if (job.active() || job.status == "queued") && ui.button("Pause").clicked()
                        {
                            self.action(job.id.clone(), "pause");
                        }
                        if job.status == "queued" && ui.button("Move up").clicked() {
                            self.action(job.id.clone(), "up");
                        }
                    });
                });
                ui.add_space(10.0);
                let fraction = if job.status == "completed" {
                    1.0
                } else {
                    job.size
                        .filter(|n| *n > 0)
                        .map(|n| job.received as f32 / n as f32)
                        .unwrap_or(0.0)
                };
                ui.add(
                    egui::ProgressBar::new(fraction.min(1.0))
                        .desired_width(ui.available_width())
                        .show_percentage(),
                );
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&job.status).color(if job.status == "error" {
                        Color32::from_rgb(190, 50, 50)
                    } else {
                        Color32::from_rgb(85, 80, 170)
                    }));
                    ui.label(format!(
                        "{} / {}",
                        bytes(job.received as f64),
                        job.size
                            .map(|n| bytes(n as f64))
                            .unwrap_or_else(|| "unknown size".into())
                    ));
                    if job.active() {
                        ui.label(format!(
                            "{} /s · {} connections",
                            bytes(job.speed),
                            job.connections
                        ));
                        if job.speed > 0.0
                            && let Some(size) = job.size
                        {
                            ui.label(format!(
                                "{}s left",
                                (size.saturating_sub(job.received) as f64 / job.speed).ceil()
                                    as u64
                            ));
                        }
                    }
                    if let Some(at) = job.scheduled_at
                        && job.status == "queued"
                    {
                        ui.label(format!(
                            "Scheduled {}",
                            at.with_timezone(&Local).format("%Y-%m-%d %H:%M")
                        ));
                    }
                });
                if let Some(error) = &job.error {
                    ui.colored_label(Color32::from_rgb(190, 50, 50), error);
                }
            });
        ui.add_space(8.0);
    }
}
impl eframe::App for Desktop {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.collect_events(ctx);
        if let Some(marker) = &self.smoke {
            for event in ctx.input(|input| input.events.clone()) {
                if let egui::Event::Screenshot {
                    image, user_data, ..
                } = event
                {
                    let suffix = user_data
                        .data
                        .as_ref()
                        .and_then(|data| data.downcast_ref::<String>())
                        .cloned()
                        .unwrap_or_else(|| "main".into());
                    let path = marker.with_extension(format!("{suffix}.png"));
                    let pixels: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
                    if let Err(error) = image::save_buffer(
                        path,
                        &pixels,
                        image.size[0] as u32,
                        image.size[1] as u32,
                        image::ColorType::Rgba8,
                    ) {
                        self.message = Some(error.to_string());
                    }
                }
            }
            match self.frames {
                4 => ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::new(
                    "main".to_string(),
                ))),
                7 => self.new_download(),
                9 => ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::new(
                    "add".to_string(),
                ))),
                12 => {
                    self.show_add = false;
                    self.view = "Settings".into();
                }
                14 => ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(
                    egui::UserData::new("settings".to_string()),
                )),
                _ => (),
            }
        }
        let state = self.engine.snapshot();
        egui::SidePanel::left("navigation")
            .resizable(false)
            .exact_width(215.0)
            .show(ctx, |ui| {
                ui.add_space(20.0);
                ui.heading(RichText::new("Relay").size(32.0));
                ui.label("DOWNLOAD MANAGER");
                ui.add_space(24.0);
                if ui
                    .add_sized([185.0, 38.0], egui::Button::new("+ New download"))
                    .clicked()
                {
                    self.new_download();
                }
                ui.add_space(20.0);
                for view in [
                    "All downloads",
                    "In progress",
                    "Scheduled",
                    "Completed",
                    "Paused & failed",
                ] {
                    ui.selectable_value(&mut self.view, view.to_string(), view);
                }
                ui.add_space(20.0);
                ui.label(RichText::new("CATEGORIES").weak());
                for view in [
                    "Videos",
                    "Music",
                    "Archives",
                    "Documents",
                    "Images",
                    "Programs",
                    "Other",
                ] {
                    ui.selectable_value(&mut self.view, view.to_string(), view);
                }
                ui.add_space(24.0);
                if ui
                    .selectable_label(self.view == "Settings", "Settings & integration")
                    .clicked()
                {
                    self.preferences = state.settings.clone();
                    self.view = "Settings".into();
                }
                ui.add_space(16.0);
                ui.label(RichText::new(format!("v{}", env!("CARGO_PKG_VERSION"))).weak());
            });
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui.button("Open download folder").clicked() {
                    let _ = std::fs::create_dir_all(&state.settings.download_dir);
                    self.open(state.settings.download_dir.clone());
                }
                ui.label(state.settings.download_dir.display().to_string());
                ui.label(if state.settings.bandwidth_k_b == 0 {
                    "Unlimited bandwidth".into()
                } else {
                    format!("{} KB/s cap", state.settings.bandwidth_k_b)
                });
            });
        });
        egui::CentralPanel::default().show(ctx, |ui| {
            if let Some(message) = self.message.clone() {
                ui.horizontal_wrapped(|ui| {
                    ui.label(message);
                    if ui.small_button("Dismiss").clicked() {
                        self.message = None;
                    }
                });
                ui.separator();
            }
            if self.view == "Settings" {
                self.settings_ui(ui);
                return;
            }
            ui.add_space(12.0);
            ui.heading(&self.view);
            ui.label(RichText::new("Everything you need. Moving at your speed.").weak());
            ui.add_space(18.0);
            ui.horizontal(|ui| {
                let running = state.jobs.iter().filter(|j| j.active()).count();
                let completed = state
                    .jobs
                    .iter()
                    .filter(|j| j.status == "completed")
                    .count();
                ui.label(
                    RichText::new(format!(
                        "{} /s",
                        bytes(state.jobs.iter().map(|j| j.speed).sum())
                    ))
                    .size(24.0),
                );
                ui.separator();
                ui.label(format!("{running} active"));
                ui.separator();
                ui.label(format!("{completed} completed"));
                ui.separator();
                ui.label(format!(
                    "{} saved",
                    bytes(
                        state
                            .jobs
                            .iter()
                            .filter(|j| j.status == "completed")
                            .map(|j| j.received as f64)
                            .sum()
                    )
                ));
            });
            ui.add_space(18.0);
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.search)
                        .hint_text("Search downloads…")
                        .desired_width(240.0),
                );
                egui::ComboBox::from_id_salt("sort")
                    .selected_text(["Newest first", "Name", "Size", "Queue order"][self.sort])
                    .show_ui(ui, |ui| {
                        for (i, name) in ["Newest first", "Name", "Size", "Queue order"]
                            .iter()
                            .enumerate()
                        {
                            ui.selectable_value(&mut self.sort, i, *name);
                        }
                    });
                egui::ComboBox::from_id_salt("queue")
                    .selected_text(if self.queue_filter.is_empty() {
                        "All queues"
                    } else {
                        &self.queue_filter
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.queue_filter, String::new(), "All queues");
                        let mut queues: Vec<_> =
                            state.jobs.iter().map(|j| j.queue.clone()).collect();
                        queues.sort();
                        queues.dedup();
                        for queue in queues {
                            ui.selectable_value(&mut self.queue_filter, queue.clone(), queue);
                        }
                    });
                if ui.button("Pause all").clicked() {
                    for j in &state.jobs {
                        if j.active() || j.status == "queued" {
                            self.action(j.id.clone(), "pause");
                        }
                    }
                }
                if ui.button("Resume all").clicked() {
                    for j in &state.jobs {
                        if matches!(j.status.as_str(), "paused" | "error") {
                            self.action(j.id.clone(), "resume");
                        }
                    }
                }
            });
            ui.add_space(14.0);
            let search = self.search.to_lowercase();
            let mut jobs: Vec<_> = state
                .jobs
                .iter()
                .filter(|j| {
                    let view_match = match self.view.as_str() {
                        "All downloads" => true,
                        "In progress" => j.active(),
                        "Scheduled" => j.status == "queued" && j.scheduled_at.is_some(),
                        "Completed" => j.status == "completed",
                        "Paused & failed" => matches!(j.status.as_str(), "paused" | "error"),
                        _ => j.category == self.view,
                    };
                    view_match
                        && (self.queue_filter.is_empty() || self.queue_filter == j.queue)
                        && format!("{} {} {}", j.name, j.url, j.queue)
                            .to_lowercase()
                            .contains(&search)
                })
                .collect();
            match self.sort {
                0 => jobs.reverse(),
                1 => jobs.sort_by_key(|j| j.name.to_lowercase()),
                2 => jobs.sort_by_key(|j| std::cmp::Reverse(j.size.unwrap_or(0))),
                _ => (),
            }
            ui.label(
                RichText::new(format!("{} of {} downloads", jobs.len(), state.jobs.len())).weak(),
            );
            egui::ScrollArea::vertical().show(ui, |ui| {
                if jobs.is_empty() {
                    ui.add_space(60.0);
                    ui.vertical_centered(|ui| {
                        ui.heading("Your next download starts here.");
                        ui.label("Add a direct link. Relay takes care of the rest.");
                        if ui.button("Add a download").clicked() {
                            self.new_download();
                        }
                    });
                }
                for job in jobs {
                    ui.push_id(&job.id, |ui| self.download_row(ui, job));
                }
            });
        });
        if self.show_add {
            self.add_ui(ctx);
        }
        if let Some(path) = &self.smoke {
            self.frames += 1;
            if self.frames >= 18 {
                let _ = std::fs::write(
                    path,
                    format!(
                        "Relay {} native UI rendered\n{}\n",
                        env!("CARGO_PKG_VERSION"),
                        assets::FILES.len()
                    ),
                );
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
        ctx.request_repaint_after(Duration::from_millis(100));
    }
}
fn bytes(value: f64) -> String {
    if value <= 0.0 {
        return "0 B".into();
    }
    let mut n = value;
    let mut i = 0;
    let units = ["B", "KB", "MB", "GB", "TB"];
    while n >= 1024.0 && i < 4 {
        n /= 1024.0;
        i += 1;
    }
    format!("{n:.1} {}", units[i])
}
