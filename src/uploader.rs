use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::watch::{Receiver, Sender};
use tracing::{error, info};
use yup_oauth2::InstalledFlowAuthenticator;
use google_youtube3::hyper_rustls::HttpsConnector;
use google_youtube3::hyper_util;
use google_youtube3::hyper_util::client::legacy::connect::HttpConnector;
use http_body_util::combinators::BoxBody;
use bytes::Bytes;

use crate::AppWindow;
use crate::storage::{AppStorage, PrivacyStatus, save_storage};
use crate::video::{get_pending_clips, merge_multiple_videos, process_video_file, path_to_string};

pub type GoogleClient = google_youtube3::hyper_util::client::legacy::Client<
    HttpsConnector<HttpConnector>,
    BoxBody<Bytes, google_youtube3::hyper::Error>,
>;

pub async fn run_background_uploader(
    path_rx: &mut Receiver<Option<PathBuf>>,
    path_tx: Arc<Sender<Option<PathBuf>>>,
    mut del_rx: Receiver<bool>,
    mut ps_rx: Receiver<PrivacyStatus>,
    storage: Arc<Mutex<AppStorage>>,
    ui_weak: slint::Weak<AppWindow>,
) {
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .expect("Fehler bei der Initialisierung von rustls");
    let secret = yup_oauth2::read_application_secret("client_secret.json")
        .await
        .expect("client_secret konnte nicht gelesen werden");

    let auth = InstalledFlowAuthenticator::builder(
        secret,
        yup_oauth2::InstalledFlowReturnMethod::HTTPRedirect,
    )
    .persist_tokens_to_disk("token_cache.json")
    .build()
    .await
    .expect("Fehler bei der Authentisierung");

    let scopes = &["https://www.googleapis.com/auth/youtube.upload"];
    auth.token(scopes)
        .await
        .expect("Fehler bei der Anmeldung im Browser");

    let connector = google_youtube3::hyper_rustls::HttpsConnectorBuilder::new()
        .with_native_roots()
        .expect("Zertifikat fehlerhaft")
        .https_only()
        .enable_http1()
        .build();

    let client: GoogleClient =
        hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
            .build(connector);

    let hub = google_youtube3::api::YouTube::new(client, auth);

    let mut uploads_today = 0;
    let mut last_upload = chrono::DateTime::default();
    if let Ok(storage_ok) = storage.lock() {
        last_upload = storage_ok.last_upload_date;
        uploads_today = storage_ok.uploads_today;
    }

    let mut interval = tokio::time::interval(std::time::Duration::from_secs(3 * 60 * 60));
    interval.tick().await;

    loop {
        let now = chrono::Local::now();

        if now.date_naive() != last_upload.date_naive() {
            info!("Neuer Tag; Upload auf 0");
            uploads_today = 0;
            last_upload = now;
            if let Ok(mut storage_ok) = storage.lock() {
                storage_ok.last_upload_date = now;
                storage_ok.uploads_today = uploads_today;
            }
            save_storage(&storage);
        }
        if now - last_upload >= chrono::Duration::hours(3) {
            let clip_folder = {
                let active_path = path_rx.borrow_and_update().clone();
                if active_path.is_none() {
                    info!("Warten auf Pfadauswahl im UI...");
                    if path_rx.changed().await.is_err() {
                        return;
                    }
                    continue;
                }
                active_path.unwrap()
            };

            if tokio::fs::read_dir(&clip_folder).await.is_err() {
                error!("Ordner nicht gefunden, Pfad zurückgesetzt");
                {
                    let mut guard = storage.lock().expect("Fehler auf AppStorage zuzugreifen");
                    guard.clip_location = None;
                }
                let storage_clone = Arc::clone(&storage);
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
                continue;
            }

            info!("Prüfe Ordner: {:?}", clip_folder);
            let uploaded_files_clone = {
                let guard = storage.lock().expect("Fehler auf AppStorage zuzugreifen");
                guard.uploaded_files.clone()
            };

            let pending_clips = get_pending_clips(&clip_folder, &uploaded_files_clone).await;

            if uploads_today >= 6 {
                info!("Upload Limit für heute erreicht (6/6). Warte auf nächsten Tag.");
            } else if !pending_clips.is_empty() {
                info!(
                    "Es wurden {} Clips zum hochladen gefunden",
                    pending_clips.len()
                );

                let chunk_size = usize::max(1, pending_clips.len() / (6 - uploads_today));
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

                    if !merge_multiple_videos(clip_paket, combined_output).await {
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

                    let mut video = google_youtube3::api::Video::default();
                    let mut details = google_youtube3::api::VideoSnippet::default();
                    details.title = Some(format!(
                        "Clip Compilation - {}",
                        path_to_string(&clip_paket[0])
                    ));

                    let names: Vec<String> = clip_paket.iter().map(path_to_string).collect();
                    let names_combined = names.join(", ");
                    details.description = Some(format!(
                        "Clip Compilation von den Videodateien: {}",
                        names_combined
                    ));

                    details.category_id = Some("22".to_string());
                    video.snippet = Some(details);

                    let mut video_status = google_youtube3::api::VideoStatus::default();
                    video_status.privacy_status =
                        Some(ps_rx.borrow().to_useable_string().to_string());
                    video.status = Some(video_status);

                    let upload_result =
                        upload_video(video, final_processed_video.to_path_buf(), &hub, video_file)
                            .await;

                    if upload_result.is_ok() {
                        for clip in clip_paket {
                            let should_delete = *del_rx.borrow();
                            {
                                let mut guard =
                                    storage.lock().expect("Fehler beim Aufruf von AppStorage");
                                guard.uploaded_files.push(path_to_string(clip));
                            }

                            if should_delete {
                                let _ = tokio::fs::remove_file(clip).await;
                            }
                        }
                        uploads_today += 1;
                        last_upload = chrono::Local::now();
                        if let Ok(mut storage_a) = storage.lock() {
                            storage_a.uploads_today = uploads_today;
                            storage_a.last_upload_date = last_upload;
                        };
                        save_storage(&storage);
                    } else {
                        error!(
                            "Upload fehlgeschlagen. Clips werden nicht als 'hochgeladen' markiert."
                        );
                    }
                }
            } else {
                info!("Keine neuen Clips gefunden");
            }
        } else {
            let remaining_time = chrono::Duration::hours(3) - (now - last_upload);
            info!(
                "Letzter Upload ist erst {} Minuten her.",
                remaining_time.num_minutes()
            );
        }

        info!("Warte auf den nächsten Interval (3h) oder eine Pfadänderung im UI...");
        tokio::select! {
            _ = interval.tick() => {
                info!("3 Stunden sind um. Starte geplante Überprüfung...");
            }
            changed_res = path_rx.changed() => {
                if changed_res.is_err() {
                    return;
                }
                info!("Pfad wurde im UI geändert! Breche Warten ab und starte sofort neue Prüfung.");
                interval.reset();
            }
            Ok(_) = del_rx.changed() => {
                let new_val = {*del_rx.borrow()};
                let storage_clone = Arc::clone(&storage);
                tokio::task::spawn_blocking(move || {
                    if let Ok(mut guard) = storage_clone.lock() {
                        guard.delete_original = new_val;
                    }
                    save_storage(&storage_clone);
                });
            }
            Ok(_) = ps_rx.changed() => {
                let new_val = {*ps_rx.borrow()};
                let storage_clone = Arc::clone(&storage);
                tokio::task::spawn_blocking(move || {
                    if let Ok(mut guard) = storage_clone.lock() {
                        guard.privacy_status = new_val;
                    }
                    save_storage(&storage_clone);
                });
            }
        }
    }
}

async fn upload_video(
    video: google_youtube3::api::Video,
    file_path: PathBuf,
    hub: &google_youtube3::YouTube<HttpsConnector<HttpConnector>>,
    video_file: tokio::fs::File,
) -> Result<(), ()> {
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
            let video_id = video.id.unwrap_or_default();
            info!("Video Link: https://www.youtube.com/watch?v={}", video_id);
            Ok(())
        }
        Err(error) => {
            error!(error = ?error, "Ein Fehler ist aufgetreten:");
            Err(())
        }
    }
}
