use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::watch::{Receiver, Sender};
use tracing::info;
use tracing_subscriber::layer::SubscriberExt;

slint::include_modules!();

mod storage;
mod logger;
mod ui;
mod video;
mod uploader;

use storage::{load_storage, AppStorage, PrivacyStatus};
use ui::setup_ui;
use uploader::run_background_uploader;
use logger::SlintLayer;

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
        log_rx,
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
        log_rx,
    );

    rt.spawn(async move {
        run_background_uploader(&mut path_rx, path_tx, del_rx, ps_rx, storage, ui_weak).await;
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
    tokio::sync::mpsc::Receiver<LogEntry>,
) {
    let (current_path, current_delete_original, current_privacy_status) = {
        let guard = storage.lock().expect("Fehler beim Lesen vom Storage");
        (
            guard.clip_location.clone(),
            guard.delete_original,
            guard.privacy_status,
        )
    };

    let (path_tx, path_rx) = tokio::sync::watch::channel(current_path);
    let path_tx = Arc::new(path_tx);

    let (del_tx, del_rx) = tokio::sync::watch::channel(current_delete_original);
    let del_tx = Arc::new(del_tx);

    let (ps_tx, ps_rx) = tokio::sync::watch::channel(current_privacy_status);
    let ps_tx = Arc::new(ps_tx);

    let (log_tx, log_rx) = tokio::sync::mpsc::channel::<LogEntry>(128);
    let subscriber = tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(SlintLayer { sender: log_tx });

    tracing::subscriber::set_global_default(subscriber).unwrap();

    (
        current_delete_original,
        current_privacy_status,
        path_rx,
        path_tx,
        del_rx,
        del_tx,
        ps_rx,
        ps_tx,
        log_rx,
    )
}
