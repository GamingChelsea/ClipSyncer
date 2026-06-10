use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tracing::info;

#[derive(Serialize, Deserialize, Default, Debug, Clone, Copy)]
pub enum PrivacyStatus {
    Public,
    Unlisted,
    #[default]
    Private,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Default)]
pub enum VideoEncoder {
    #[default]
    Auto,
    Cpu,
    Nvidia,
    Amd,
    Intel,
}

impl VideoEncoder {
    pub fn new(index: i32) -> Self {
        match index {
            0 => VideoEncoder::Auto,
            1 => VideoEncoder::Cpu,
            2 => VideoEncoder::Nvidia,
            3 => VideoEncoder::Amd,
            4 => VideoEncoder::Intel,
            _ => VideoEncoder::Auto,
        }
    }
}

impl PrivacyStatus {
    pub fn new(index: i32) -> Self {
        match index {
            0 => PrivacyStatus::Public,
            1 => PrivacyStatus::Unlisted,
            _ => PrivacyStatus::Private,
        }
    }

    pub fn to_useable_string(&self) -> &str {
        match self {
            Self::Public => "public",
            Self::Unlisted => "unlisted",
            Self::Private => "private",
        }
    }
}

#[derive(Serialize, Deserialize, Default, Debug)]
pub struct AppStorage {
    pub clip_location: Option<PathBuf>,
    pub uploaded_files: Vec<String>,
    pub uploaded_videos: Vec<google_youtube3::api::Video>,
    pub delete_original: bool,
    pub upload_all: bool,
    pub privacy_status: PrivacyStatus,
    pub last_upload_date: chrono::DateTime<chrono::Local>,
    pub uploads_today: usize,
    pub video_encoder: VideoEncoder,
}

pub fn save_storage(storage: &Arc<Mutex<AppStorage>>) {
    let storage_guard = storage.lock().expect("Fehler beim Storage Guard");
    let ron_string = ron::ser::to_string_pretty(&*storage_guard, ron::ser::PrettyConfig::default())
        .expect("Fehler bei der RON Konvertierung");
    std::fs::write("config.ron", ron_string).expect("Fehler bei der Config Speicherung");
    info!("Gespeichert");
}

pub fn load_storage() -> Arc<Mutex<AppStorage>> {
    let mut output = AppStorage {
        clip_location: None,
        uploaded_files: vec![],
        uploaded_videos: vec![],
        delete_original: false,
        upload_all: false,
        privacy_status: PrivacyStatus::Unlisted,
        last_upload_date: chrono::Local::now() - chrono::Duration::hours(4),
        uploads_today: 0,
        video_encoder: VideoEncoder::Auto,
    };
    let ron_content = std::fs::read_to_string("config.ron");
    match ron_content {
        Ok(content) => {
            output = ron::from_str(&content).unwrap_or_default();
        }
        _ => {}
    }
    Arc::new(Mutex::new(output))
}
