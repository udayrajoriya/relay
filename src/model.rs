use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub download_dir: PathBuf,
    pub connections: usize,
    pub simultaneous: usize,
    pub bandwidth_k_b: u64,
    pub organize: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            download_dir: PathBuf::new(),
            connections: 6,
            simultaneous: 2,
            bandwidth_k_b: 0,
            organize: true,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Part {
    pub start: u64,
    pub end: Option<i64>,
    pub done: u64,
}
impl Part {
    pub fn length(&self) -> Option<u64> {
        self.end.map(|end| {
            if end < 0 {
                0
            } else {
                (end as u64 + 1).saturating_sub(self.start)
            }
        })
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    pub id: String,
    pub url: String,
    pub name: String,
    #[serde(default)]
    pub custom_name: bool,
    pub queue: String,
    pub scheduled_at: Option<DateTime<Utc>>,
    pub max_connections: usize,
    pub status: String,
    pub size: Option<u64>,
    pub received: u64,
    #[serde(default)]
    pub speed: f64,
    pub parts: Vec<Part>,
    pub created_at: DateTime<Utc>,
    pub error: Option<String>,
    pub category: String,
    pub destination: PathBuf,
    #[serde(default)]
    pub parts_in_destination: bool,
    pub organize: bool,
    #[serde(default)]
    pub ranges: bool,
    pub validator: Option<String>,
    pub file_path: Option<PathBuf>,
    pub completed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_url_update_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub connections: usize,
}
impl Job {
    pub fn active(&self) -> bool {
        matches!(
            self.status.as_str(),
            "probing" | "downloading" | "assembling"
        )
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct Snapshot {
    pub settings: Settings,
    pub jobs: Vec<Job>,
}
#[derive(Clone, Default, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AddInput {
    pub url: String,
    pub name: Option<String>,
    pub destination: Option<PathBuf>,
    pub queue: Option<String>,
    pub connections: Option<usize>,
    pub paused: bool,
    pub scheduled_at: Option<DateTime<Utc>>,
}
