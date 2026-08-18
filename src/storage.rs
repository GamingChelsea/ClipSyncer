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

#[derive(Serialize, Deserialize, Default, Debug, Clone)]
#[serde(default)]
pub struct CachedVideo {
    pub id: String,
    pub title: String,
    pub link: String,
    pub visibility: String,
    pub thumbnail_url: String,
    #[serde(skip_serializing, default)]
    pub thumbnail_bytes: Option<Vec<u8>>,
}

fn deserialize_uploaded_videos<'de, D>(deserializer: D) -> Result<Vec<CachedVideo>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum TempVideo {
        Cached(CachedVideo),
        Legacy(google_youtube3::api::Video),
    }

    let vec = Vec::<TempVideo>::deserialize(deserializer)?;
    let converted = vec
        .into_iter()
        .map(|v| match v {
            TempVideo::Cached(c) => c,
            TempVideo::Legacy(l) => {
                let id = l.id.clone().unwrap_or_default();
                let title = l
                    .snippet
                    .as_ref()
                    .and_then(|s| s.title.clone())
                    .unwrap_or_default();
                let link = format!("https://www.youtube.com/watch?v={}", id);
                let visibility = l
                    .status
                    .as_ref()
                    .and_then(|s| s.privacy_status.clone())
                    .unwrap_or_else(|| "private".to_string());
                let thumbnail_url = l
                    .snippet
                    .as_ref()
                    .and_then(|s| s.thumbnails.as_ref())
                    .and_then(|t| t.medium.as_ref().or(t.default.as_ref()).or(t.high.as_ref()))
                    .and_then(|thumb| thumb.url.clone())
                    .unwrap_or_default();

                CachedVideo {
                    id,
                    title,
                    link,
                    visibility,
                    thumbnail_url,
                    thumbnail_bytes: None,
                }
            }
        })
        .collect();
    Ok(converted)
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(default)]
pub struct AppStorage {
    pub clip_location: Option<PathBuf>,
    pub uploaded_files: Vec<String>,
    #[serde(deserialize_with = "deserialize_uploaded_videos")]
    pub uploaded_videos: Vec<CachedVideo>,
    pub delete_original: bool,
    pub notify: bool,
    pub upload_all: bool,
    pub privacy_status: PrivacyStatus,
    pub last_upload_date: chrono::DateTime<chrono::Local>,
    pub uploads_today: usize,
    pub video_encoder: VideoEncoder,
    pub client_secret: Option<yup_oauth2::ApplicationSecret>,
    pub token_cache: Option<String>,
    pub token_cache_2: Option<String>,
    pub uploads_today_2: usize,
    pub last_upload_date_2: chrono::DateTime<chrono::Local>,
    pub active_account: usize,
    pub max_uploads_per_day: usize,
    pub language: String,
    pub autostart: bool,
}

impl Default for AppStorage {
    fn default() -> Self {
        AppStorage {
            clip_location: None,
            uploaded_files: vec![],
            uploaded_videos: vec![],
            delete_original: false,
            notify: false,
            upload_all: false,
            privacy_status: PrivacyStatus::Unlisted,
            last_upload_date: chrono::Local::now() - chrono::Duration::hours(4),
            uploads_today: 0,
            video_encoder: VideoEncoder::Auto,
            client_secret: None,
            token_cache: None,
            token_cache_2: None,
            uploads_today_2: 0,
            last_upload_date_2: chrono::Local::now() - chrono::Duration::hours(4),
            active_account: 0,
            max_uploads_per_day: 6,
            language: "en".to_string(),
            autostart: false,
        }
    }
}

pub fn get_thumbnail_cache_dir() -> PathBuf {
    let mut path = if let Ok(local_appdata) = std::env::var("LOCALAPPDATA") {
        let mut p = PathBuf::from(local_appdata);
        p.push("ClipSyncer");
        p
    } else {
        std::env::current_dir().unwrap_or_default()
    };
    path.push("thumbnails");
    let _ = std::fs::create_dir_all(&path);
    path
}

pub fn save_thumbnail_cache(video_id: &str, bytes: &[u8]) {
    let dir = get_thumbnail_cache_dir();
    let file_path = dir.join(format!("{}.jpg", video_id));
    let _ = std::fs::write(file_path, bytes);
}

pub fn load_thumbnail_cache(video_id: &str) -> Option<Vec<u8>> {
    let dir = get_thumbnail_cache_dir();
    let file_path = dir.join(format!("{}.jpg", video_id));
    std::fs::read(file_path).ok()
}

pub fn save_storage(storage: &Arc<Mutex<AppStorage>>) {
    let storage_guard = storage.lock().expect("Fehler beim Storage Guard");
    let config = ron::ser::PrettyConfig::default().compact_arrays(true);
    let ron_string = ron::ser::to_string_pretty(&*storage_guard, config)
        .expect("Fehler bei der RON Konvertierung");
    std::fs::write("config.ron", ron_string).expect("Fehler bei der Config Speicherung");
    info!("Gespeichert");
}

pub fn load_storage() -> Arc<Mutex<AppStorage>> {
    let ron_content = std::fs::read_to_string("config.ron");
    let mut output: AppStorage = match ron_content {
        Ok(content) => match ron::from_str(&content) {
            Ok(parsed) => parsed,
            Err(e) => {
                tracing::error!("Fehler beim Parsen der config.ron: {:?}", e);
                if let Err(err) = std::fs::copy("config.ron", "config.ron.bak") {
                    tracing::error!(
                        "Fehler beim Erstellen des Backups config.ron.bak: {:?}",
                        err
                    );
                } else {
                    tracing::info!(
                        "Backup der fehlerhaften Config gespeichert unter config.ron.bak"
                    );
                }
                AppStorage::default()
            }
        },
        _ => AppStorage::default(),
    };

    let mut had_legacy_bytes = false;
    for v in &mut output.uploaded_videos {
        if let Some(bytes) = &v.thumbnail_bytes {
            save_thumbnail_cache(&v.id, bytes);
            had_legacy_bytes = true;
        } else if let Some(cached) = load_thumbnail_cache(&v.id) {
            v.thumbnail_bytes = Some(cached);
        }
    }

    let storage_arc = Arc::new(Mutex::new(output));
    if had_legacy_bytes {
        save_storage(&storage_arc);
    }
    storage_arc
}

pub fn is_ffmpeg_available() -> bool {
    ffmpeg_sidecar::command::ffmpeg_is_installed()
}

pub fn download_ffmpeg_local() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    ffmpeg_sidecar::download::auto_download()?;
    Ok(())
}

#[cfg(target_os = "windows")]
pub fn set_autostart(enabled: bool) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_WRITE};
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let path = r"Software\Microsoft\Windows\CurrentVersion\Run";
    let (key, _) = hkcu.create_subkey_with_flags(path, KEY_WRITE)?;

    if enabled {
        if let Ok(exe_path) = std::env::current_exe() {
            let exe_str = exe_path.to_string_lossy();
            let value = format!("\"{}\"", exe_str);
            key.set_value("ClipSyncer", &value)?;
        }
    } else {
        let _ = key.delete_value("ClipSyncer");
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub fn set_autostart(_enabled: bool) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    Ok(())
}

