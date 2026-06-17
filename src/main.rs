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

use logger::SlintLayer;
use storage::{AppStorage, PrivacyStatus, load_storage};
use ui::setup_ui;
use uploader::run_background_uploader;

fn main() {
    let storage = load_storage();

    let (
        current_delete_original,
        current_privacy_status,
        mut path_rx,
        path_tx,
        del_rx,
        del_tx,
        ps_rx,
        ps_tx,
        active_account_rx,
        active_account_tx,
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
        current_privacy_status,
        &path_tx,
        del_tx,
        ps_tx,
        active_account_tx,
        log_rx,
        &video_tx,
        video_rx,
    );

    rt.spawn(async move {
        run_background_uploader(
            &mut path_rx,
            path_tx,
            del_rx,
            ps_rx,
            active_account_rx,
            video_tx,
            storage,
            ui_weak,
        )
        .await;
    });

    ui.show().expect("Fehler das Fenster anzuzeigen");
    slint::run_event_loop_until_quit().expect("Fehler beim Slint Event Loop");
}

fn setup_channels(
    storage: &Arc<Mutex<AppStorage>>,
) -> (
    bool,
    PrivacyStatus,
    Receiver<Option<PathBuf>>,
    Arc<Sender<Option<PathBuf>>>,
    Receiver<bool>,
    Arc<Sender<bool>>,
    Receiver<PrivacyStatus>,
    Arc<Sender<PrivacyStatus>>,
    Receiver<usize>,
    Arc<Sender<usize>>,
    tokio::sync::mpsc::Receiver<LogEntry>,
    Arc<tokio::sync::mpsc::Sender<VideoChannelEntry>>,
    tokio::sync::mpsc::Receiver<VideoChannelEntry>,
) {
    let (current_path, current_delete_original, current_privacy_status, current_active_account) = {
        let guard = storage.lock().expect("Fehler beim Lesen vom Storage");
        (
            guard.clip_location.clone(),
            guard.delete_original,
            guard.privacy_status,
            guard.active_account,
        )
    };

    let (path_tx, path_rx) = tokio::sync::watch::channel(current_path);
    let path_tx = Arc::new(path_tx);

    let (del_tx, del_rx) = tokio::sync::watch::channel(current_delete_original);
    let del_tx = Arc::new(del_tx);

    let (ps_tx, ps_rx) = tokio::sync::watch::channel(current_privacy_status);
    let ps_tx = Arc::new(ps_tx);

    let (active_account_tx, active_account_rx) = tokio::sync::watch::channel(current_active_account);
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

    (
        current_delete_original,
        current_privacy_status,
        path_rx,
        path_tx,
        del_rx,
        del_tx,
        ps_rx,
        ps_tx,
        active_account_rx,
        active_account_tx,
        log_rx,
        video_tx,
        video_rx,
    )
}
