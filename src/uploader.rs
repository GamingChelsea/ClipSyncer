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
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'a>> {
        let ui_weak = self.ui_weak.clone();
        let url_str = url.to_string();
        Box::pin(async move {
            info!("Bitte öffne den Webbrowser zur Google-Anmeldung: {}", url_str);
            
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

pub async fn run_background_uploader(
    path_rx: &mut Receiver<Option<PathBuf>>,
    path_tx: Arc<Sender<Option<PathBuf>>>,
    mut del_rx: Receiver<bool>,
    mut ps_rx: Receiver<PrivacyStatus>,
    video_tx: Arc<tokio::sync::mpsc::Sender<VideoChannelEntry>>,
    storage: Arc<Mutex<AppStorage>>,
    ui_weak: slint::Weak<AppWindow>,
) {
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .expect("Fehler bei der Initialisierung von rustls");

    let mut secret_opt;
    let mut initial_token_json;

    loop {
        {
            let guard = storage.lock().expect("Fehler auf AppStorage zuzugreifen");
            secret_opt = guard.client_secret.clone();
            initial_token_json = guard.token_cache.clone();
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

    let temp_token_file =
        tempfile::NamedTempFile::new().expect("Fehler beim Erstellen der temporären Token-Datei");
    let temp_path = temp_token_file.path().to_path_buf();

    let token_content = initial_token_json.unwrap_or_else(|| "[]".to_string());
    tokio::fs::write(&temp_path, token_content)
        .await
        .expect("Fehler beim Schreiben des Token-Caches");

    let auth = InstalledFlowAuthenticator::builder(
        secret,
        yup_oauth2::InstalledFlowReturnMethod::HTTPRedirect,
    )
    .flow_delegate(Box::new(SlintOAuthDelegate {
        ui_weak: ui_weak.clone(),
    }))
    .persist_tokens_to_disk(&temp_path)
    .build()
    .await
    .expect("Fehler bei der Authentisierung");

    let _ = &ui_weak.upgrade().map(|ui| ui.set_logged_in(false));

    let scopes = &[
        "https://www.googleapis.com/auth/youtube.upload",
        "https://www.googleapis.com/auth/youtube.readonly",
    ];
    auth.token(scopes)
        .await
        .expect("Fehler bei der Anmeldung im Browser");

    let ui_weak_login = ui_weak.clone();

    let _ = slint::invoke_from_event_loop(move || {
        if let Some(ui) = ui_weak_login.upgrade() {
            ui.set_logged_in(true);
            ui.set_login_url("".into());
        }
    });

    if let Ok(updated_tokens) = tokio::fs::read_to_string(&temp_path).await {
        {
            let mut guard = storage.lock().expect("Fehler auf AppStorage zuzugreifen");
            guard.token_cache = Some(updated_tokens);
        }
        save_storage(&storage);
    }

    let connector = google_youtube3::hyper_rustls::HttpsConnectorBuilder::new()
        .with_native_roots()
        .expect("Zertifikat fehlerhaft")
        .https_only()
        .enable_http1()
        .build();

    let client: GoogleClient =
        hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
            .build(connector);

    let auth_clone = auth.clone();
    let hub = google_youtube3::api::YouTube::new(client, auth);

    let missing_ids: Vec<String> = {
        let guard = storage.lock().expect("Fehler beim Lesen vom Storage");
        guard.uploaded_videos.iter()
            .filter(|v| v.thumbnail_bytes.is_none())
            .map(|v| v.id.clone())
            .collect()
    };

    if !missing_ids.is_empty() {
        info!("Hintergrund-Task zum Nachladen von {} fehlenden Vorschaubildern gestartet...", missing_ids.len());
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
                                    let thumbnail_url = snippet.thumbnails.as_ref()
                                        .and_then(|t| t.medium.as_ref().or(t.default.as_ref()).or(t.high.as_ref()))
                                        .and_then(|thumb| thumb.url.clone())
                                        .unwrap_or_default();
                                    
                                    if !thumbnail_url.is_empty() {
                                        let mut token_str = None;
                                        if let Ok(token_res) = auth_clone.token(scopes).await {
                                            token_str = token_res.token().map(String::from);
                                        }
                                        
                                        let mut req = client.get(&thumbnail_url);
                                        if let Some(ref t) = token_str {
                                            req = req.bearer_auth(t);
                                        }
                                        
                                        match req.send().await {
                                            Ok(resp) if resp.status().is_success() => {
                                                if let Ok(bytes) = resp.bytes().await {
                                                    let bytes_vec = bytes.to_vec();
                                                    info!("Thumbnail für Video {} erfolgreich nachgeladen ({} Bytes)", video_id, bytes_vec.len());
                                                    
                                                    let mut updated_entry = None;
                                                    if let Ok(mut guard) = storage.lock() {
                                                        if let Some(v) = guard.uploaded_videos.iter_mut().find(|x| x.id == *video_id) {
                                                            v.thumbnail_bytes = Some(bytes_vec.clone());
                                                            v.thumbnail_url = thumbnail_url.clone();
                                                            updated_entry = Some(VideoChannelEntry {
                                                                title: v.title.clone(),
                                                                link: v.link.clone(),
                                                                visibility: v.visibility.clone(),
                                                                thumbnail_url: thumbnail_url.clone(),
                                                                thumbnail_bytes: Some(bytes_vec),
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
                                                error!("Fehler beim Download des Thumbnails für {} (Status {})", video_id, resp.status());
                                            }
                                            Err(e) => {
                                                error!("Fehler beim Senden des Thumbnail-Requests für {}: {:?}", video_id, e);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    error!("Fehler beim Abrufen der Video-Details für Thumbnail-Update: {:?}", e);
                }
            }
        }
    }

    let _ = path_rx.borrow_and_update();
    let _ = del_rx.borrow_and_update();
    let _ = ps_rx.borrow_and_update();

    info!("Führe erste Clip-Überprüfung beim App-Start aus...");
    perform_check_and_upload(
        &hub, &auth_clone, path_rx, &path_tx, &del_rx, &ps_rx, &video_tx, &storage, &ui_weak, false,
    )
    .await;

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
                perform_check_and_upload(&hub, &auth_clone, path_rx, &path_tx, &del_rx, &ps_rx, &video_tx, &storage, &ui_weak, true).await;
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

            res = ps_rx.changed() => {
                if res.is_err() {
                    return;
                }
                let new_val = *ps_rx.borrow_and_update();
                info!("Sichtbarkeit im UI geändert auf: {:?}", new_val);

                let storage_clone = Arc::clone(&storage);
                tokio::task::spawn_blocking(move || {
                    if let Ok(mut guard) = storage_clone.lock() {
                        guard.privacy_status = new_val;
                    }
                    save_storage(&storage_clone);
                });
            }

            _ = interval.tick() => {
                info!("3 Stunden sind um. Starte geplante automatische Überprüfung...");
                perform_check_and_upload(&hub, &auth_clone, path_rx, &path_tx, &del_rx, &ps_rx, &video_tx, &storage, &ui_weak, false).await;
            }
        }
    }
}

async fn perform_check_and_upload(
    hub: &google_youtube3::YouTube<HttpsConnector<HttpConnector>>,
    auth: &yup_oauth2::authenticator::Authenticator<HttpsConnector<HttpConnector>>,
    path_rx: &mut Receiver<Option<PathBuf>>,
    path_tx: &Arc<Sender<Option<PathBuf>>>,
    del_rx: &Receiver<bool>,
    ps_rx: &Receiver<PrivacyStatus>,
    video_tx: &Arc<tokio::sync::mpsc::Sender<VideoChannelEntry>>,
    storage: &Arc<Mutex<AppStorage>>,
    ui_weak: &slint::Weak<AppWindow>,
    ignore_time_limit: bool,
) {
    let now = chrono::Local::now();
    let mut uploads_today = 0;
    let mut last_upload = chrono::DateTime::default();
    let mut upload_all = false;

    if let Ok(mut storage_ok) = storage.lock() {
        last_upload = storage_ok.last_upload_date;
        uploads_today = storage_ok.uploads_today;
        upload_all = storage_ok.upload_all;
        if upload_all {
            storage_ok.upload_all = false;
        }
    }

    if now.date_naive() != last_upload.date_naive() {
        info!("Neuer Tag; Upload auf 0");
        uploads_today = 0;
        last_upload = now;
        if let Ok(mut storage_ok) = storage.lock() {
            storage_ok.last_upload_date = now;
            storage_ok.uploads_today = uploads_today;
        }
        save_storage(storage);
    }

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

    if uploads_today >= 6 {
        info!("Upload Limit für heute erreicht (6/6). Warte auf nächsten Tag.");
    } else if !pending_clips.is_empty() {
        info!(
            "Es wurden {} Clips zum hochladen gefunden",
            pending_clips.len()
        );

        let mut chunk_size = usize::max(1, pending_clips.len() / (6 - uploads_today));
        if upload_all {
            chunk_size = pending_clips.len();
        }
        for clip_paket in pending_clips.chunks(chunk_size) {
            if uploads_today >= 6 {
                break;
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

            let upload_result =
                upload_video(video, final_processed_video.to_path_buf(), hub, video_file).await;
            if let Ok(video) = upload_result {
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
                uploads_today += 1;
                let current_time = chrono::Local::now();
                let video_id = video.id.clone().unwrap_or_default();
                let link = format!("https://www.youtube.com/watch?v={}", video_id);
                let mut title = String::from("Fehler");
                let mut privacy = PrivacyStatus::Private;
                if let Some(snippet) = &video.snippet {
                    title = snippet.title.clone().unwrap_or_default();
                };
                if let Some(visibility) = video.status.as_ref().and_then(|s| s.privacy_status.as_deref()) {
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
                            if let Some(token_str) = token_res.token() {
                                match client.get(&thumbnail_url)
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
                                            title, attempt, max_attempts
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
                    storage_a.uploads_today = uploads_today;
                    storage_a.last_upload_date = current_time;
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
                save_storage(storage);
            } else {
                error!("Upload fehlgeschlagen. Clips werden nicht als 'hochgeladen' markiert.");
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
) -> Result<google_youtube3::api::Video, ()> {
    info!(
        "Starte Upload von der Datei: {:?}:...",
        file_path.file_name()
    );

    let result = hub
        .videos()
        .insert(video)
        .upload(
            video_file.into_std().await,
            "video/*".parse().expect("Fehler beim Video Upload"),
        )
        .await;

    match result {
        Ok((_response, video)) => {
            info!("Video wurde erfolgreich hochgeladen");
            let video_id = video.id.clone().unwrap_or_default();
            info!("Video Link: https://www.youtube.com/watch?v={}", video_id);
            Ok(video)
        }
        Err(error) => {
            error!(error = ?error, "Ein Fehler ist aufgetreten:");
            Err(())
        }
    }
}
