use bytes::Bytes;
use google_youtube3::hyper_rustls::HttpsConnector;
use google_youtube3::hyper_util;
use google_youtube3::hyper_util::client::legacy::connect::HttpConnector;
use http_body_util::combinators::BoxBody;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::watch::{Receiver, Sender};
use tracing::{error, info};
use yup_oauth2::InstalledFlowAuthenticator;
use yup_oauth2::authenticator_delegate::InstalledFlowDelegate;

struct SlintOAuthDelegate {
    ui_weak: slint::Weak<AppWindow>,
}

impl InstalledFlowDelegate for SlintOAuthDelegate {
    fn present_user_url<'a>(
        &'a self,
        url: &'a str,
        need_code: bool,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'a>>
    {
        let ui_weak = self.ui_weak.clone();
        let url_str = url.to_string();
        Box::pin(async move {
            info!(
                "Bitte öffne den Webbrowser zur Google-Anmeldung: {}",
                url_str
            );

            // Versuche den Browser automatisch zu öffnen
            if let Err(e) = webbrowser::open(&url_str) {
                error!("Fehler beim automatischen Öffnen des Webbrowsers: {}", e);
            }

            // Setze die URL in der Slint UI
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = ui_weak.upgrade() {
                    ui.set_login_url(url_str.into());
                }
            });

            if need_code {
                Err("Manuelle Code-Eingabe wird nicht unterstützt.".to_string())
            } else {
                Ok(String::new())
            }
        })
    }
}

use crate::storage::{AppStorage, PrivacyStatus, VideoEncoder, save_storage};
use crate::video::{get_pending_clips, merge_multiple_videos, path_to_string, process_video_file};
use crate::{AppWindow, VideoChannelEntry};

pub type GoogleClient = google_youtube3::hyper_util::client::legacy::Client<
    HttpsConnector<HttpConnector>,
    BoxBody<Bytes, google_youtube3::hyper::Error>,
>;

#[derive(Debug)]
pub enum UploadError {
    LimitExceeded,
    Other,
}

fn is_upload_limit_exceeded(err: &google_youtube3::Error) -> bool {
    if let google_youtube3::Error::BadRequest(value) = err {
        if let Some(error_obj) = value.get("error") {
            if let Some(errors_arr) = error_obj.get("errors").and_then(|e| e.as_array()) {
                for error_entry in errors_arr {
                    if let Some(reason) = error_entry.get("reason").and_then(|r| r.as_str()) {
                        if reason == "uploadLimitExceeded" {
                            return true;
                        }
                    }
                }
            }
        }
    }
    false
}

async fn save_tokens(
    storage: &Arc<Mutex<AppStorage>>,
    temp_path: &std::path::Path,
    active_account: usize,
) {
    if let Ok(updated_tokens) = tokio::fs::read_to_string(temp_path).await {
        {
            let mut guard = storage.lock().expect("Fehler auf AppStorage zuzugreifen");
            if active_account == 0 {
                guard.token_cache = Some(updated_tokens);
            } else {
                guard.token_cache_2 = Some(updated_tokens);
            }
        }
        save_storage(storage);
    }
}

async fn get_youtube_client(
    storage: &Arc<Mutex<AppStorage>>,
    ui_weak: &slint::Weak<AppWindow>,
    secret: yup_oauth2::ApplicationSecret,
    temp_path: &std::path::Path,
    active_account: usize,
) -> Option<(
    google_youtube3::YouTube<HttpsConnector<HttpConnector>>,
    yup_oauth2::authenticator::Authenticator<HttpsConnector<HttpConnector>>,
)> {
    let token_content = {
        let guard = storage.lock().expect("Fehler auf AppStorage zuzugreifen");
        let cache = if active_account == 0 {
            &guard.token_cache
        } else {
            &guard.token_cache_2
        };
        match cache {
            Some(s) if !s.trim().is_empty() => s.clone(),
            _ => "[]".to_string(),
        }
    };

    if let Err(e) = tokio::fs::write(temp_path, token_content).await {
        error!("Fehler beim Schreiben des Token-Caches: {:?}", e);
        return None;
    }

    let ui_weak_login = ui_weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(ui) = ui_weak_login.upgrade() {
            ui.set_logged_in(false);
        }
    });

    let auth = match InstalledFlowAuthenticator::builder(
        secret,
        yup_oauth2::InstalledFlowReturnMethod::HTTPRedirect,
    )
    .flow_delegate(Box::new(SlintOAuthDelegate {
        ui_weak: ui_weak.clone(),
    }))
    .persist_tokens_to_disk(temp_path)
    .build()
    .await
    {
        Ok(a) => a,
        Err(e) => {
            error!("Fehler bei der Authentisierung: {:?}", e);
            return None;
        }
    };

    let scopes = &[
        "https://www.googleapis.com/auth/youtube.upload",
        "https://www.googleapis.com/auth/youtube.readonly",
    ];

    match auth.token(scopes).await {
        Ok(_) => {
            let ui_weak_login = ui_weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = ui_weak_login.upgrade() {
                    ui.set_logged_in(true);
                    ui.set_login_url("".into());
                }
            });

            save_tokens(storage, temp_path, active_account).await;

            let ui_weak_limit = ui_weak.clone();
            let storage_clone = Arc::clone(storage);
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = ui_weak_limit.upgrade() {
                    if let Ok(guard) = storage_clone.lock() {
                        crate::ui::update_max_uploads_limit(&ui, &guard);
                    }
                }
            });

            let connector = google_youtube3::hyper_rustls::HttpsConnectorBuilder::new()
                .with_native_roots()
                .expect("Zertifikat fehlerhaft")
                .https_only()
                .enable_http1()
                .build();

            let client: GoogleClient =
                hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
                    .build(connector);

            let hub = google_youtube3::api::YouTube::new(client, auth.clone());
            Some((hub, auth))
        }
        Err(e) => {
            error!("Fehler bei der Anmeldung im Browser: {:?}", e);
            None
        }
    }
}

fn is_account_logged_in(storage: &Arc<Mutex<AppStorage>>, account_idx: usize) -> bool {
    let guard = storage.lock().expect("Fehler");
    let cache = if account_idx == 0 {
        &guard.token_cache
    } else {
        &guard.token_cache_2
    };
    match cache {
        Some(s) if !s.trim().is_empty() && s.trim() != "[]" => true,
        _ => false,
    }
}

fn determine_automatic_account(storage: &Arc<Mutex<AppStorage>>) -> usize {
    let guard = storage.lock().expect("Fehler beim Lesen");
    if guard.uploads_today >= 6 { 1 } else { 0 }
}

async fn switch_account(
    hub: &mut google_youtube3::YouTube<HttpsConnector<HttpConnector>>,
    auth: &mut yup_oauth2::authenticator::Authenticator<HttpsConnector<HttpConnector>>,
    temp_path: &mut PathBuf,
    active_account: &mut usize,
    secret: &yup_oauth2::ApplicationSecret,
    temp_dir_path: &PathBuf,
    storage: &Arc<Mutex<AppStorage>>,
    ui_weak: &slint::Weak<AppWindow>,
    new_account: usize,
) -> bool {
    info!("Wechsle zu Account {}", new_account + 1);

    let selected_account = {
        let guard = storage.lock().expect("Fehler auf AppStorage zuzugreifen");
        guard.active_account
    };

    if selected_account != 2 {
        {
            let mut guard = storage.lock().expect("Fehler auf AppStorage zuzugreifen");
            guard.active_account = new_account;
        }
        save_storage(storage);
    }

    *active_account = new_account;
    *temp_path = if new_account == 0 {
        temp_dir_path.join("tokens_1.json")
    } else {
        temp_dir_path.join("tokens_2.json")
    };

    if let Some((new_hub, new_auth)) =
        get_youtube_client(storage, ui_weak, secret.clone(), temp_path, new_account).await
    {
        *hub = new_hub;
        *auth = new_auth;

        if selected_account != 2 {
            let ui_weak_clone = ui_weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = ui_weak_clone.upgrade() {
                    ui.set_active_account_index(new_account as i32);
                }
            });
        }

        true
    } else {
        error!(
            "Fehler beim Wechseln des YouTube-Clients zu Account {}",
            new_account + 1
        );
        false
    }
}

fn update_next_check_ui(
    ui_weak: &slint::Weak<AppWindow>,
    next_check: chrono::DateTime<chrono::Local>,
) {
    let text = format!(
        "Nächste automatische Prüfung: {} Uhr",
        next_check.format("%H:%M")
    );
    let ui_clone = ui_weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(ui) = ui_clone.upgrade() {
            ui.set_next_check_text(text.into());
        }
    });
}

pub async fn run_background_uploader(
    path_rx: &mut Receiver<Option<PathBuf>>,
    path_tx: Arc<Sender<Option<PathBuf>>>,
    mut del_rx: Receiver<bool>,
    mut not_rx: Receiver<bool>,
    mut ps_rx: Receiver<PrivacyStatus>,
    mut active_account_rx: Receiver<usize>,
    video_tx: Arc<tokio::sync::mpsc::Sender<VideoChannelEntry>>,
    storage: Arc<Mutex<AppStorage>>,
    ui_weak: slint::Weak<AppWindow>,
) {
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .expect("Fehler bei der Initialisierung von rustls");

    let mut secret_opt;

    loop {
        {
            let guard = storage.lock().expect("Fehler auf AppStorage zuzugreifen");
            secret_opt = guard.client_secret.clone();
        }

        if secret_opt.is_some() {
            break;
        }

        let ui_weak_clone = ui_weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = ui_weak_clone.upgrade() {
                ui.set_needs_client_secret(true);
            }
        });

        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    }

    let secret = secret_opt.unwrap();

    let temp_dir = tempfile::tempdir().expect("Fehler beim Erstellen des temporären Ordners");
    let temp_dir_path = temp_dir.path().to_path_buf();

    let selected_account = {
        let guard = storage.lock().expect("Fehler beim Lesen");
        guard.active_account
    };

    if selected_account == 2 && !is_account_logged_in(&storage, 1) {
        info!(
            "Automatisch-Modus aktiv, aber Zweitaccount ist noch nicht angemeldet. Starte Anmeldung für Zweitaccount..."
        );
        let temp_path_2 = temp_dir_path.join("tokens_2.json");
        if let Some((_hub, _auth)) =
            get_youtube_client(&storage, &ui_weak, secret.clone(), &temp_path_2, 1).await
        {
            info!("Zweitaccount erfolgreich angemeldet.");
        } else {
            error!("Anmeldung für Zweitaccount fehlgeschlagen.");
        }
    }

    let mut active_account = if selected_account == 2 {
        determine_automatic_account(&storage)
    } else {
        selected_account
    };

    let mut temp_path = if active_account == 0 {
        temp_dir_path.join("tokens_1.json")
    } else {
        temp_dir_path.join("tokens_2.json")
    };

    let (mut hub, mut auth) = loop {
        if let Some(res) = get_youtube_client(
            &storage,
            &ui_weak,
            secret.clone(),
            &temp_path,
            active_account,
        )
        .await
        {
            break res;
        }
        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
    };

    let missing_ids: Vec<String> = {
        let guard = storage.lock().expect("Fehler beim Lesen vom Storage");
        guard
            .uploaded_videos
            .iter()
            .filter(|v| v.thumbnail_bytes.is_none())
            .map(|v| v.id.clone())
            .collect()
    };

    if !missing_ids.is_empty() {
        info!(
            "Hintergrund-Task zum Nachladen von {} fehlenden Vorschaubildern gestartet...",
            missing_ids.len()
        );
        let client = reqwest::Client::new();
        let scopes = &[
            "https://www.googleapis.com/auth/youtube.upload",
            "https://www.googleapis.com/auth/youtube.readonly",
        ];
        for chunk in missing_ids.chunks(50) {
            let parts = vec!["snippet".to_string()];
            let mut request = hub.videos().list(&parts);
            for id in chunk {
                request = request.add_id(id);
            }
            match request.doit().await {
                Ok((_response, video_list)) => {
                    if let Some(items) = video_list.items {
                        for video in items {
                            if let Some(video_id) = &video.id {
                                if let Some(snippet) = &video.snippet {
                                    let thumbnail_url = snippet
                                        .thumbnails
                                        .as_ref()
                                        .and_then(|t| {
                                            t.medium
                                                .as_ref()
                                                .or(t.default.as_ref())
                                                .or(t.high.as_ref())
                                        })
                                        .and_then(|thumb| thumb.url.clone())
                                        .unwrap_or_default();

                                    if !thumbnail_url.is_empty() {
                                        let mut token_str = None;
                                        if let Ok(token_res) = auth.token(scopes).await {
                                            token_str = token_res.token().map(String::from);
                                            save_tokens(&storage, &temp_path, active_account).await;
                                        }

                                        let mut req = client.get(&thumbnail_url);
                                        if let Some(ref t) = token_str {
                                            req = req.bearer_auth(t);
                                        }

                                        match req.send().await {
                                            Ok(resp) if resp.status().is_success() => {
                                                if let Ok(bytes) = resp.bytes().await {
                                                    let bytes_vec = bytes.to_vec();
                                                    info!(
                                                        "Thumbnail für Video {} erfolgreich nachgeladen ({} Bytes)",
                                                        video_id,
                                                        bytes_vec.len()
                                                    );

                                                    let mut updated_entry = None;
                                                    if let Ok(mut guard) = storage.lock() {
                                                        if let Some(v) = guard
                                                            .uploaded_videos
                                                            .iter_mut()
                                                            .find(|x| x.id == *video_id)
                                                        {
                                                            v.thumbnail_bytes =
                                                                Some(bytes_vec.clone());
                                                            v.thumbnail_url = thumbnail_url.clone();
                                                            updated_entry =
                                                                Some(VideoChannelEntry {
                                                                    title: v.title.clone(),
                                                                    link: v.link.clone(),
                                                                    visibility: v
                                                                        .visibility
                                                                        .clone(),
                                                                    thumbnail_url: thumbnail_url
                                                                        .clone(),
                                                                    thumbnail_bytes: Some(
                                                                        bytes_vec,
                                                                    ),
                                                                });
                                                        }
                                                    }
                                                    if let Some(entry) = updated_entry {
                                                        save_storage(&storage);
                                                        let _ = video_tx.send(entry).await;
                                                    }
                                                }
                                            }
                                            Ok(resp) => {
                                                error!(
                                                    "Fehler beim Download des Thumbnails für {} (Status {})",
                                                    video_id,
                                                    resp.status()
                                                );
                                            }
                                            Err(e) => {
                                                error!(
                                                    "Fehler beim Senden des Thumbnail-Requests für {}: {:?}",
                                                    video_id, e
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    error!(
                        "Fehler beim Abrufen der Video-Details für Thumbnail-Update: {:?}",
                        e
                    );
                }
            }
        }
    }

    let _ = path_rx.borrow_and_update();
    let _ = del_rx.borrow_and_update();
    let _ = not_rx.borrow_and_update();
    let _ = ps_rx.borrow_and_update();
    let _ = active_account_rx.borrow_and_update();

    info!("Führe erste Clip-Überprüfung beim App-Start aus...");
    perform_check_and_upload(
        &mut hub,
        &mut auth,
        &mut temp_path,
        &mut active_account,
        &secret,
        &temp_dir_path,
        path_rx,
        &path_tx,
        &del_rx,
        &not_rx,
        &ps_rx,
        &video_tx,
        &storage,
        &ui_weak,
        false,
    )
    .await;
    let next_check = chrono::Local::now() + chrono::Duration::hours(3);
    update_next_check_ui(&ui_weak, next_check);

    let mut interval = tokio::time::interval(std::time::Duration::from_secs(3 * 60 * 60));
    interval.tick().await;

    loop {
        info!("Warte auf den nächsten Intervall (3h) oder eine Änderung im UI...");
        tokio::select! {
            changed_res = path_rx.changed() => {
                if changed_res.is_err() {
                    return;
                }
                let _new_path = path_rx.borrow_and_update().clone();
                info!("Pfad wurde im UI geändert! Breche Warten ab und starte sofort neue Prüfung.");
                interval.reset();
                perform_check_and_upload(
                    &mut hub,
                    &mut auth,
                    &mut temp_path,
                    &mut active_account,
                    &secret,
                    &temp_dir_path,
                    path_rx,
                    &path_tx,
                    &del_rx,
                    &not_rx,
                    &ps_rx,
                    &video_tx,
                    &storage,
                    &ui_weak,
                    true,
                ).await;
                let next_check = chrono::Local::now() + chrono::Duration::hours(3);
                update_next_check_ui(&ui_weak, next_check);
            }

            res = del_rx.changed() => {
                if res.is_err() {
                    return;
                }
                let new_val = *del_rx.borrow_and_update();
                info!("Einstellung 'delete_original' im UI geändert auf: {}", new_val);

                let storage_clone = Arc::clone(&storage);
                tokio::task::spawn_blocking(move || {
                    if let Ok(mut guard) = storage_clone.lock() {
                        guard.delete_original = new_val;
                    }
                    save_storage(&storage_clone);
                });
            }

            res = not_rx.changed() => {
                if res.is_err() {
                    return;
                }
                let new_val = *not_rx.borrow_and_update();
                info!("Einstellung 'Benachrichtigung' im UI geändert auf: {}", new_val);

                let storage_clone = Arc::clone(&storage);
                tokio::task::spawn_blocking(move || {
                    if let Ok(mut guard) = storage_clone.lock() {
                        guard.notify = new_val;
                    }
                    save_storage(&storage_clone);
                });
            }

            res = active_account_rx.changed() => {
                if res.is_err() {
                    return;
                }
                let new_selected = *active_account_rx.borrow_and_update();

                if new_selected == 2 && !is_account_logged_in(&storage, 1) {
                    info!("Automatisch-Modus im UI ausgewählt, aber Zweitaccount ist noch nicht angemeldet. Starte Anmeldung für Zweitaccount...");
                    let temp_path_2 = temp_dir_path.join("tokens_2.json");
                    if let Some((_hub, _auth)) = get_youtube_client(&storage, &ui_weak, secret.clone(), &temp_path_2, 1).await {
                        info!("Zweitaccount erfolgreich angemeldet.");
                    } else {
                        error!("Anmeldung für Zweitaccount fehlgeschlagen.");
                    }
                }

                let new_account = if new_selected == 2 {
                    determine_automatic_account(&storage)
                } else {
                    new_selected
                };



                if new_account != active_account {
                    info!("Aktiver Account im UI geändert auf: {}", new_account + 1);
                    if switch_account(
                        &mut hub,
                        &mut auth,
                        &mut temp_path,
                        &mut active_account,
                        &secret,
                        &temp_dir_path,
                        &storage,
                        &ui_weak,
                        new_account,
                    ).await {
                        info!("Account-Wechsel erfolgreich. Starte sofort neue Prüfung.");
                        interval.reset();
                        perform_check_and_upload(
                            &mut hub,
                            &mut auth,
                            &mut temp_path,
                            &mut active_account,
                            &secret,
                            &temp_dir_path,
                            path_rx,
                            &path_tx,
                            &del_rx,
                            &not_rx,
                            &ps_rx,
                            &video_tx,
                            &storage,
                            &ui_weak,
                            true,
                        ).await;
                        let next_check = chrono::Local::now() + chrono::Duration::hours(3);
                        update_next_check_ui(&ui_weak, next_check);
                    }
                }
            }

            _ = interval.tick() => {
                info!("3 Stunden sind um. Starte geplante automatische Überprüfung...");
                perform_check_and_upload(
                    &mut hub,
                    &mut auth,
                    &mut temp_path,
                    &mut active_account,
                    &secret,
                    &temp_dir_path,
                    path_rx,
                    &path_tx,
                    &del_rx,
                    &not_rx,
                    &ps_rx,
                    &video_tx,
                    &storage,
                    &ui_weak,
                    false,
                ).await;
                let next_check = chrono::Local::now() + chrono::Duration::hours(3);
                update_next_check_ui(&ui_weak, next_check);
            }
        }
    }
}

async fn perform_check_and_upload(
    hub: &mut google_youtube3::YouTube<HttpsConnector<HttpConnector>>,
    auth: &mut yup_oauth2::authenticator::Authenticator<HttpsConnector<HttpConnector>>,
    temp_path: &mut PathBuf,
    active_account: &mut usize,
    secret: &yup_oauth2::ApplicationSecret,
    temp_dir_path: &PathBuf,
    path_rx: &mut Receiver<Option<PathBuf>>,
    path_tx: &Arc<Sender<Option<PathBuf>>>,
    del_rx: &Receiver<bool>,
    not_rx: &Receiver<bool>,
    ps_rx: &Receiver<PrivacyStatus>,
    video_tx: &Arc<tokio::sync::mpsc::Sender<VideoChannelEntry>>,
    storage: &Arc<Mutex<AppStorage>>,
    ui_weak: &slint::Weak<AppWindow>,
    ignore_time_limit: bool,
) {
    let now = chrono::Local::now();
    let mut upload_all = false;

    let mut date_changed = false;
    if let Ok(mut storage_ok) = storage.lock() {
        if now.date_naive() != storage_ok.last_upload_date.date_naive() {
            info!("Neuer Tag; Setze Upload-Limit für Account 1 zurück");
            storage_ok.uploads_today = 0;
            storage_ok.last_upload_date = now;
            date_changed = true;
        }
        if now.date_naive() != storage_ok.last_upload_date_2.date_naive() {
            info!("Neuer Tag; Setze Upload-Limit für Account 2 zurück");
            storage_ok.uploads_today_2 = 0;
            storage_ok.last_upload_date_2 = now;
            date_changed = true;
        }
        upload_all = storage_ok.upload_all;
        if upload_all {
            storage_ok.upload_all = false;
        }
    }
    if date_changed {
        save_storage(storage);
    }

    let selected_account = {
        let guard = storage.lock().expect("Fehler");
        guard.active_account
    };

    let (uploads_today, uploads_today_2, max_limit) = {
        let guard = storage.lock().expect("Fehler");
        (
            guard.uploads_today,
            guard.uploads_today_2,
            guard.max_uploads_per_day,
        )
    };

    let total_uploads_today = uploads_today + uploads_today_2;
    if total_uploads_today >= max_limit {
        info!(
            "Tägliches Gesamtuploadlimit von {} erreicht. Warte auf nächsten Tag.",
            max_limit
        );
        return;
    }

    let mut check_limit = true;
    while check_limit {
        let current_uploads_today = {
            let guard = storage.lock().expect("Fehler");
            if *active_account == 0 {
                guard.uploads_today
            } else {
                guard.uploads_today_2
            }
        };

        if current_uploads_today >= 6 {
            if selected_account == 2 && *active_account == 0 {
                info!(
                    "Hauptaccount hat sein Limit erreicht. Wechsle automatisch auf Zweitaccount (Modus: Automatisch)."
                );
                if switch_account(
                    hub,
                    auth,
                    temp_path,
                    active_account,
                    secret,
                    temp_dir_path,
                    storage,
                    ui_weak,
                    1,
                )
                .await
                {
                    continue;
                }
            } else {
                if *active_account == 0 {
                    info!("Hauptaccount hat sein Limit erreicht.");
                } else {
                    info!("Zweitaccount hat sein Limit erreicht.");
                }
            }
        }
        check_limit = false;
    }

    let last_upload = {
        let guard = storage.lock().expect("Fehler");
        if *active_account == 0 {
            guard.last_upload_date
        } else {
            guard.last_upload_date_2
        }
    };

    let bypass_time_limit = ignore_time_limit || upload_all;

    if !bypass_time_limit && (now - last_upload < chrono::Duration::hours(3)) {
        let remaining_time = chrono::Duration::hours(3) - (now - last_upload);
        info!(
            "Letzter Upload ist erst {} Minuten her.",
            remaining_time.num_minutes()
        );
        return;
    }

    let clip_folder = {
        let active_path = path_rx.borrow().clone();
        if active_path.is_none() {
            info!("Warten auf Pfadauswahl im UI...");
            return;
        }
        active_path.unwrap()
    };

    if tokio::fs::read_dir(&clip_folder).await.is_err() {
        error!("Ordner nicht gefunden, Pfad zurückgesetzt");
        {
            let mut guard = storage.lock().expect("Fehler auf AppStorage zuzugreifen");
            guard.clip_location = None;
        }
        let storage_clone = Arc::clone(storage);
        tokio::task::spawn_blocking(move || save_storage(&storage_clone))
            .await
            .unwrap();

        let _ = path_tx.send_replace(None);
        let ui_weak_clone = ui_weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = ui_weak_clone.upgrade() {
                ui.set_selected_path("Kein Pfad ausgewählt".into());
            }
        });
        return;
    }

    info!("Prüfe Ordner: {:?}", clip_folder);

    let mut uploaded_files_clone = Vec::<String>::new();
    let mut video_encoder = VideoEncoder::Auto;
    if let Ok(storage_ok) = storage.lock() {
        uploaded_files_clone = storage_ok.uploaded_files.clone();
        video_encoder = storage_ok.video_encoder;
    }

    let pending_clips = get_pending_clips(&clip_folder, &uploaded_files_clone).await;

    if !pending_clips.is_empty() {
        info!(
            "Es wurden {} Clips zum hochladen gefunden",
            pending_clips.len()
        );

        let (uploads_today_curr, uploads_today_2_curr, max_limit_curr) = {
            let guard = storage.lock().expect("Fehler");
            (
                guard.uploads_today,
                guard.uploads_today_2,
                guard.max_uploads_per_day,
            )
        };
        let total_uploads_today_curr = uploads_today_curr + uploads_today_2_curr;
        let remaining_total_slots = if max_limit_curr > total_uploads_today_curr {
            max_limit_curr - total_uploads_today_curr
        } else {
            0
        };
        let remaining_active_slots = if *active_account == 0 {
            if uploads_today_curr < 6 {
                6 - uploads_today_curr
            } else {
                0
            }
        } else {
            if uploads_today_2_curr < 6 {
                6 - uploads_today_2_curr
            } else {
                0
            }
        };
        let remaining_slots = usize::min(remaining_total_slots, remaining_active_slots);
        if remaining_slots == 0 {
            info!("Keine verbleibenden Upload-Slots für heute.");
            return;
        }

        let mut chunk_size = usize::max(1, pending_clips.len() / remaining_slots);
        if upload_all {
            chunk_size = pending_clips.len();
        }
        for clip_paket in pending_clips.chunks(chunk_size) {
            let (mut current_uploads_today, total_uploads, max_limit) = {
                let guard = storage.lock().expect("Fehler");
                (
                    if *active_account == 0 {
                        guard.uploads_today
                    } else {
                        guard.uploads_today_2
                    },
                    guard.uploads_today + guard.uploads_today_2,
                    guard.max_uploads_per_day,
                )
            };

            if total_uploads >= max_limit {
                info!(
                    "Tägliches Gesamtuploadlimit von {} erreicht. Breche Upload-Loop ab.",
                    max_limit
                );
                break;
            }

            if current_uploads_today >= 6 {
                if selected_account == 2 && *active_account == 0 {
                    info!(
                        "Hauptaccount-Limit im Loop erreicht. Wechsle auf Zweitaccount (Modus: Automatisch)."
                    );
                    if switch_account(
                        hub,
                        auth,
                        temp_path,
                        active_account,
                        secret,
                        temp_dir_path,
                        storage,
                        ui_weak,
                        1,
                    )
                    .await
                    {
                        let guard = storage.lock().expect("Fehler");
                        current_uploads_today = guard.uploads_today_2;
                    } else {
                        break;
                    }
                }

                if current_uploads_today >= 6 {
                    info!("Aktiver Account voll. Breche Upload-Loop ab.");
                    break;
                }
            }

            info!("Verarbeite ein Paket von {} Clips...", clip_paket.len());

            let combined_output_temp = match tempfile::Builder::new()
                .prefix("batch_combined_")
                .suffix(".mp4")
                .tempfile()
            {
                Ok(file) => file,
                Err(e) => {
                    error!(error = ?e, "Konnte temporäre Batch Datei nicht erstellen");
                    continue;
                }
            };
            let combined_output = combined_output_temp.path();

            if !merge_multiple_videos(clip_paket, combined_output, video_encoder).await {
                error!("Paket Verarbeitung wegen FFmpeg-Merge-Fehler abgebrochen");
                continue;
            }

            let final_processed_temp = match tempfile::Builder::new()
                .prefix("batch_final_")
                .suffix(".mp4")
                .tempfile()
            {
                Ok(file) => file,
                Err(e) => {
                    error!(error = ?e, "Konnte temporäre Final Datei nicht erstellen");
                    continue;
                }
            };

            let final_processed_video = final_processed_temp.path();

            if !process_video_file(combined_output, final_processed_video).await {
                error!("Paket Verarbeitung wegen FFmpeg Faststart Fehler abgebrochen");
                continue;
            }

            let video_file = match tokio::fs::File::open(final_processed_video).await {
                Ok(file) => file,
                Err(e) => {
                    error!(video = ?final_processed_video, error = ?e, "Datei konnte nicht geöffnet werden");
                    continue;
                }
            };

            let names: Vec<String> = clip_paket.iter().map(|p| path_to_string(p)).collect();
            let names_combined = names.join(", ");
            let details = google_youtube3::api::VideoSnippet {
                title: Some(format!(
                    "Clip Compilation - {}",
                    path_to_string(&clip_paket[0])
                )),
                description: Some(format!(
                    "Clip Compilation von den Videodateien: {}",
                    names_combined
                )),
                category_id: Some("22".to_string()),
                ..Default::default()
            };

            let video_status = google_youtube3::api::VideoStatus {
                privacy_status: Some(ps_rx.borrow().to_useable_string().to_string()),
                ..Default::default()
            };

            let video = google_youtube3::api::Video {
                snippet: Some(details),
                status: Some(video_status),
                ..Default::default()
            };

            let upload_result = upload_video(
                video,
                final_processed_video.to_path_buf(),
                hub,
                video_file,
                ui_weak,
            )
            .await;
            match upload_result {
                Ok(video) => {
                    handle_successful_upload(
                        video,
                        clip_paket,
                        storage,
                        del_rx,
                        not_rx,
                        video_tx,
                        auth,
                        temp_path,
                        *active_account,
                    )
                    .await;
                }
                Err(UploadError::LimitExceeded) => {
                    info!("API meldet Limit überschritten.");
                    {
                        let mut guard = storage.lock().expect("Fehler");
                        if *active_account == 0 {
                            guard.uploads_today = 6;
                        } else {
                            guard.uploads_today_2 = 6;
                        }
                    }
                    save_storage(storage);

                    if selected_account == 2 && *active_account == 0 {
                        info!(
                            "Wechsle automatisch zu Zweitaccount für einen erneuten Upload-Versuch (Modus: Automatisch)."
                        );
                        if switch_account(
                            hub,
                            auth,
                            temp_path,
                            active_account,
                            secret,
                            temp_dir_path,
                            storage,
                            ui_weak,
                            1,
                        )
                        .await
                        {
                            let (total_uploads, max_limit) = {
                                let guard = storage.lock().expect("Fehler");
                                (
                                    guard.uploads_today + guard.uploads_today_2,
                                    guard.max_uploads_per_day,
                                )
                            };
                            if total_uploads >= max_limit {
                                info!("Gesamtuploadlimit erreicht vor Retry. Breche ab.");
                                break;
                            }

                            let retry_video_file = match tokio::fs::File::open(
                                final_processed_video,
                            )
                            .await
                            {
                                Ok(file) => file,
                                Err(e) => {
                                    error!(video = ?final_processed_video, error = ?e, "Datei konnte nicht erneut geöffnet werden");
                                    continue;
                                }
                            };

                            let details = google_youtube3::api::VideoSnippet {
                                title: Some(format!(
                                    "Clip Compilation - {}",
                                    path_to_string(&clip_paket[0])
                                )),
                                description: Some(format!(
                                    "Clip Compilation von den Videodateien: {}",
                                    names_combined
                                )),
                                category_id: Some("22".to_string()),
                                ..Default::default()
                            };

                            let video_status = google_youtube3::api::VideoStatus {
                                privacy_status: Some(
                                    ps_rx.borrow().to_useable_string().to_string(),
                                ),
                                ..Default::default()
                            };

                            let video = google_youtube3::api::Video {
                                snippet: Some(details),
                                status: Some(video_status),
                                ..Default::default()
                            };

                            let retry_result = upload_video(
                                video,
                                final_processed_video.to_path_buf(),
                                hub,
                                retry_video_file,
                                ui_weak,
                            )
                            .await;
                            if let Ok(video) = retry_result {
                                handle_successful_upload(
                                    video,
                                    clip_paket,
                                    storage,
                                    del_rx,
                                    not_rx,
                                    video_tx,
                                    auth,
                                    temp_path,
                                    *active_account,
                                )
                                .await;
                                continue;
                            }
                        }
                    }
                    error!(
                        "Upload fehlgeschlagen wegen Limitüberschreitung und kein weiterer Account im automatischen Modus verfügbar."
                    );
                }
                Err(UploadError::Other) => {
                    error!("Upload fehlgeschlagen. Clips werden nicht als 'hochgeladen' markiert.");
                }
            }
        }
    } else {
        info!("Keine neuen Clips gefunden");
    }
}

async fn upload_video(
    video: google_youtube3::api::Video,
    file_path: PathBuf,
    hub: &google_youtube3::YouTube<HttpsConnector<HttpConnector>>,
    video_file: tokio::fs::File,
    ui_weak: &slint::Weak<AppWindow>,
) -> Result<google_youtube3::api::Video, UploadError> {
    info!(
        "Starte Upload von der Datei: {:?}:...",
        file_path.file_name()
    );

    let ui_weak_clone = ui_weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(ui) = ui_weak_clone.upgrade() {
            ui.set_is_uploading(true);
            ui.set_upload_progress(0.0);
        }
    });

    let mut delegate = UploadDelegate {
        ui_weak: ui_weak.clone(),
    };

    let result = hub
        .videos()
        .insert(video)
        .delegate(&mut delegate)
        .upload(
            video_file.into_std().await,
            "video/*".parse().expect("Fehler beim Video Upload"),
        )
        .await;

    let ui_weak_clone2 = ui_weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(ui) = ui_weak_clone2.upgrade() {
            ui.set_is_uploading(false);
            ui.set_upload_progress(0.0);
        }
    });

    match result {
        Ok((_response, video)) => {
            info!("Video wurde erfolgreich hochgeladen");
            let video_id = video.id.clone().unwrap_or_default();
            info!("Video Link: https://www.youtube.com/watch?v={}", video_id);
            Ok(video)
        }
        Err(error) => {
            error!(error = ?error, "Ein Fehler ist aufgetreten:");
            if is_upload_limit_exceeded(&error) {
                Err(UploadError::LimitExceeded)
            } else {
                Err(UploadError::Other)
            }
        }
    }
}

async fn handle_successful_upload(
    video: google_youtube3::api::Video,
    clip_paket: &[PathBuf],
    storage: &Arc<Mutex<AppStorage>>,
    del_rx: &Receiver<bool>,
    not_rx: &Receiver<bool>,
    video_tx: &Arc<tokio::sync::mpsc::Sender<VideoChannelEntry>>,
    auth: &yup_oauth2::authenticator::Authenticator<HttpsConnector<HttpConnector>>,
    temp_path: &std::path::Path,
    active_account: usize,
) {
    for clip in clip_paket {
        let should_delete = *del_rx.borrow();
        {
            let mut guard = storage.lock().expect("Fehler beim Aufruf von AppStorage");
            guard.uploaded_files.push(path_to_string(clip));
        }

        if should_delete {
            let _ = tokio::fs::remove_file(clip).await;
        }
    }

    let current_time = chrono::Local::now();
    let video_id = video.id.clone().unwrap_or_default();
    let link = format!("https://www.youtube.com/watch?v={}", video_id);
    let mut title = String::from("Fehler");
    let mut privacy = PrivacyStatus::Private;
    if let Some(snippet) = &video.snippet {
        title = snippet.title.clone().unwrap_or_default();
    };
    if let Some(visibility) = video
        .status
        .as_ref()
        .and_then(|s| s.privacy_status.as_deref())
    {
        match visibility {
            "public" => privacy = PrivacyStatus::Public,
            "unlisted" => privacy = PrivacyStatus::Unlisted,
            _ => privacy = PrivacyStatus::Private,
        }
    }
    let thumbnail_url = video
        .snippet
        .as_ref()
        .and_then(|s| s.thumbnails.as_ref())
        .and_then(|t| t.medium.as_ref().or(t.default.as_ref()).or(t.high.as_ref()))
        .and_then(|m| m.url.as_ref())
        .cloned()
        .unwrap_or_default();

    let mut thumbnail_bytes = None;
    if !thumbnail_url.is_empty() {
        let client = reqwest::Client::new();
        let scopes = &[
            "https://www.googleapis.com/auth/youtube.upload",
            "https://www.googleapis.com/auth/youtube.readonly",
        ];
        let max_attempts = 10;
        let delay = std::time::Duration::from_secs(10);

        for attempt in 1..=max_attempts {
            if let Ok(token_res) = auth.token(scopes).await {
                save_tokens(storage, temp_path, active_account).await;
                if let Some(token_str) = token_res.token() {
                    match client
                        .get(&thumbnail_url)
                        .bearer_auth(token_str)
                        .send()
                        .await
                    {
                        Ok(response) if response.status().is_success() => {
                            if let Ok(bytes) = response.bytes().await {
                                thumbnail_bytes = Some(bytes.to_vec());
                                break;
                            }
                        }
                        _ => {
                            tracing::info!(
                                "Thumbnail für {} noch nicht bereit (Versuch {}/{}).",
                                title,
                                attempt,
                                max_attempts
                            );
                        }
                    }
                }
            }
            if attempt < max_attempts {
                tokio::time::sleep(delay).await;
            }
        }
    }

    let video_entry = VideoChannelEntry {
        title: title.clone(),
        link: link.clone(),
        visibility: privacy.to_useable_string().to_string(),
        thumbnail_url: thumbnail_url.clone(),
        thumbnail_bytes: thumbnail_bytes.clone(),
    };
    let _ = video_tx.send(video_entry).await;
    if let Ok(mut storage_a) = storage.lock() {
        if active_account == 0 {
            storage_a.uploads_today += 1;
            storage_a.last_upload_date = current_time;
        } else {
            storage_a.uploads_today_2 += 1;
            storage_a.last_upload_date_2 = current_time;
        }
        storage_a.uploaded_videos.push(crate::storage::CachedVideo {
            id: video_id,
            title,
            link,
            visibility: privacy.to_useable_string().to_string(),
            thumbnail_url,
            thumbnail_bytes,
        });
        storage_a.upload_all = false;
    };

    let should_send_notification = *not_rx.borrow();
    if should_send_notification {
        let _ = notify_rust::Notification::new()
            .summary("Erfolgreich hochgeladen") // Die Überschrift
            .body("Clip wurde erfolgreich auf YouTube hochgeladen.")
            .appname("Clip Syncer")
            .show();
    }
    save_storage(storage);
}

struct UploadDelegate {
    ui_weak: slint::Weak<AppWindow>,
}

impl google_youtube3::Delegate for UploadDelegate {
    fn cancel_chunk_upload(&mut self, chunk: &google_youtube3::common::ContentRange) -> bool {
        if let Some(range) = &chunk.range {
            let total = chunk.total_length as f32;
            if total > 0.0 {
                let uploaded = (range.first) as f32;
                let percent = (uploaded / total) * 100.0;
                let ui_weak = self.ui_weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = ui_weak.upgrade() {
                        ui.set_upload_progress(percent);
                    }
                });
            }
        }
        false
    }

    fn chunk_size(&mut self) -> u64 {
        1 << 18 // 256 KB
    }
}
