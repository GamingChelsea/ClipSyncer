#![windows_subsystem = "windows"]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::watch::{Receiver, Sender};
use tracing::info;
use tracing_subscriber::layer::SubscriberExt;

slint::include_modules!();

#[derive(Debug, Clone)]
pub struct VideoChannelEntry {
    pub title: String,
    pub link: String,
    pub visibility: String,
    pub thumbnail_url: String,
    pub thumbnail_bytes: Option<Vec<u8>>,
}
mod logger;
mod storage;
mod ui;
mod uploader;
mod video;
mod i18n;

use logger::SlintLayer;
use storage::{AppStorage, PrivacyStatus, load_storage};
use ui::setup_ui;
use uploader::run_background_uploader;

fn main() {
    if let Ok(local_appdata) = std::env::var("LOCALAPPDATA") {
        let mut path = PathBuf::from(local_appdata);
        path.push("ClipSyncer");
        let _ = std::fs::create_dir_all(&path);
        let _ = std::env::set_current_dir(&path);
    }

    rustls::crypto::ring::default_provider().install_default().expect("Failed to install TLS provider");
    let storage = load_storage();

    let autostart_enabled = {
        let guard = storage.lock().expect("Fehler beim Lesen");
        guard.autostart
    };
    if let Err(e) = storage::set_autostart(autostart_enabled) {
        tracing::error!("Fehler beim Synchronisieren des Autostarts bei Startup: {:?}", e);
    }

    let (
        current_delete_original,
        current_notify,
        current_privacy_status,
        mut path_rx,
        path_tx,
        del_rx,
        del_tx,
        not_rx,
        not_tx,
        ps_rx,
        ps_tx,
        active_account_rx,
        active_account_tx,
        cancel_rx,
        cancel_tx,
        log_rx,
        video_tx,
        video_rx,
    ) = setup_channels(&storage);

    info!("App startet");

    let rt = tokio::runtime::Runtime::new().expect("Tokio Runtime Fehler");
    let _guard = rt.enter();

    let (ui, ui_weak, _tray) = setup_ui(
        &storage,
        current_delete_original,
        current_notify,
        current_privacy_status,
        &path_tx,
        del_tx,
        not_tx,
        ps_tx,
        active_account_tx,
        &cancel_tx,
        log_rx,
        &video_tx,
        video_rx,
    );

    let ui_weak_uploader = ui_weak.clone();
    let storage_uploader = Arc::clone(&storage);
    rt.spawn(async move {
        run_background_uploader(
            &mut path_rx,
            path_tx,
            del_rx,
            not_rx,
            ps_rx,
            active_account_rx,
            cancel_rx,
            cancel_tx,
            video_tx,
            storage_uploader,
            ui_weak_uploader,
        )
        .await;
    });

    ui.show().expect("Fehler das Fenster anzuzeigen");

    let is_available = storage::is_ffmpeg_available();
    if !is_available {
        let language = {
            let guard = storage.lock().expect("Fehler beim Lesen");
            guard.language.clone()
        };
        let msg = if language.to_lowercase() == "de" {
            "FFmpeg wird heruntergeladen... Bitte warten."
        } else {
            "Downloading FFmpeg... Please wait."
        };
        ui.set_status_text(msg.into());
        let ui_weak_download = ui_weak.clone();
        let lang_clone = language.clone();
        std::thread::spawn(move || {
            match storage::download_ffmpeg_local() {
                Ok(_) => {
                    info!("FFmpeg erfolgreich heruntergeladen");
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = ui_weak_download.upgrade() {
                            let language = ui.get_i18n();
                            ui.set_status_text(language.status_waiting);
                        }
                    });
                }
                Err(e) => {
                    tracing::error!("Fehler beim Download von FFmpeg: {:?}", e);
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = ui_weak_download.upgrade() {
                            let err_msg = if lang_clone.to_lowercase() == "de" {
                                format!("Download-Fehler: {}. Bitte App neu starten.", e)
                            } else {
                                format!("Download error: {}. Please restart the app.", e)
                            };
                            ui.set_status_text(err_msg.into());
                        }
                    });
                }
            }
        });
    }

    slint::run_event_loop_until_quit().expect("Fehler beim Slint Event Loop");
}

fn setup_channels(
    storage: &Arc<Mutex<AppStorage>>,
) -> (
    bool,
    bool,
    PrivacyStatus,
    Receiver<Option<PathBuf>>,
    Arc<Sender<Option<PathBuf>>>,
    Receiver<bool>,
    Arc<Sender<bool>>,
    Receiver<bool>,
    Arc<Sender<bool>>,
    Receiver<PrivacyStatus>,
    Arc<Sender<PrivacyStatus>>,
    Receiver<usize>,
    Arc<Sender<usize>>,
    Receiver<bool>,
    Arc<Sender<bool>>,
    tokio::sync::mpsc::Receiver<LogEntry>,
    Arc<tokio::sync::mpsc::Sender<VideoChannelEntry>>,
    tokio::sync::mpsc::Receiver<VideoChannelEntry>,
) {
    let (
        current_path,
        current_delete_original,
        current_notify,
        current_privacy_status,
        current_active_account,
    ) = {
        let guard = storage.lock().expect("Fehler beim Lesen vom Storage");
        (
            guard.clip_location.clone(),
            guard.delete_original,
            guard.notify,
            guard.privacy_status,
            guard.active_account,
        )
    };

    let (path_tx, path_rx) = tokio::sync::watch::channel(current_path);
    let path_tx = Arc::new(path_tx);

    let (del_tx, del_rx) = tokio::sync::watch::channel(current_delete_original);
    let del_tx = Arc::new(del_tx);

    let (not_tx, not_rx) = tokio::sync::watch::channel(current_notify);
    let not_tx = Arc::new(not_tx);

    let (ps_tx, ps_rx) = tokio::sync::watch::channel(current_privacy_status);
    let ps_tx = Arc::new(ps_tx);

    let (active_account_tx, active_account_rx) =
        tokio::sync::watch::channel(current_active_account);
    let active_account_tx = Arc::new(active_account_tx);

    let (log_tx, log_rx) = tokio::sync::mpsc::channel::<LogEntry>(128);

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"))
        .add_directive("h2=off".parse().unwrap())
        .add_directive("hyper=off".parse().unwrap())
        .add_directive("hyper_util=off".parse().unwrap())
        .add_directive("google_youtube3=info".parse().unwrap());

    let subscriber = tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer())
        .with(SlintLayer { sender: log_tx });

    tracing::subscriber::set_global_default(subscriber).unwrap();

    let (video_tx, video_rx) = tokio::sync::mpsc::channel::<VideoChannelEntry>(100);
    let video_tx = Arc::new(video_tx);

    let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
    let cancel_tx = Arc::new(cancel_tx);

    let current_notify = false;
    (
        current_delete_original,
        current_notify,
        current_privacy_status,
        path_rx,
        path_tx,
        del_rx,
        del_tx,
        not_rx,
        not_tx,
        ps_rx,
        ps_tx,
        active_account_rx,
        active_account_tx,
        cancel_rx,
        cancel_tx,
        log_rx,
        video_tx,
        video_rx,
    )
}
