use bytes::Bytes;
use google_youtube3::hyper_rustls::HttpsConnector;
use google_youtube3::hyper_util;
use google_youtube3::hyper_util::client::legacy::connect::HttpConnector;
use http_body_util::combinators::BoxBody;
use serde::{Deserialize, Serialize};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use yup_oauth2::InstalledFlowAuthenticator;

slint::include_modules!();

type GoogleClient = hyper_util::client::legacy::Client<
    HttpsConnector<HttpConnector>,
    BoxBody<Bytes, google_youtube3::hyper::Error>,
>;

#[derive(Serialize, Deserialize, Default, Debug)]
struct AppStorage {
    clip_location: Option<PathBuf>,
    uploaded_files: Vec<String>,
    delete_original: bool,
}

fn main() {
    let storage = load_storage();

    let (current_path, current_delete_original) = {
        let guard = storage.lock().expect("Fehler beim Lesen vom Storage");
        (guard.clip_location.clone(), guard.delete_original.clone())
    };

    let (path_tx, mut path_rx) = tokio::sync::watch::channel(current_path);
    let path_tx = std::sync::Arc::new(path_tx);

    let (del_tx, del_rx) = tokio::sync::watch::channel(current_delete_original);
    let del_tx = std::sync::Arc::new(del_tx);

    let rt = tokio::runtime::Runtime::new().expect("Tokio Runtime Fehler");

    let image_path = "assets/icon.png";
    let dynamic_image = image::open(image_path).expect("Fehler beim öffnen vom Icon");
    let rgba_image = dynamic_image.to_rgba8();
    let (width, height) = rgba_image.dimensions();
    let raw_pixels = rgba_image.into_raw();

    let icon =
        tray_icon::Icon::from_rgba(raw_pixels, width, height).expect("Fehler beim Icon Tray");

    let tray_menu = tray_icon::menu::Menu::new();
    let open_item = tray_icon::menu::MenuItem::new("Öffnen", true, None);
    let open_item_id = open_item.id().clone();
    let quit_item = tray_icon::menu::MenuItem::new("Beenden", true, None);
    let quit_item_id = quit_item.id().clone();

    tray_menu
        .append(&open_item)
        .expect("Fehler bei der Menü Erstellung");
    tray_menu
        .append(&quit_item)
        .expect("Fehler bei der Menü Erstellung");

    let _tray_icon = tray_icon::TrayIconBuilder::new()
        .with_tooltip("Clip Syncer")
        .with_icon(icon)
        .with_menu(Box::new(tray_menu.clone()))
        .with_menu_on_left_click(false)
        .build()
        .expect("Fehler beim erstellen des Tray Icons");

    let ui = AppWindow::new().expect("Fehler beim erstellen vom UI");
    let ui_weak = ui.as_weak();

    let storage_clone = Arc::clone(&storage);
    let ui_weak_dialog = ui_weak.clone();
    let path_tx_clone = path_tx.clone();
    ui.on_open_save_dialog(move || {
        let ui_weak_for_thread = ui_weak_dialog.clone();
        let storage_for_thread = Arc::clone(&storage_clone);
        let path_tx_for_thread = path_tx_clone.clone();
        std::thread::spawn(move || {
            let result = rfd::FileDialog::new()
                .set_title("Wähle deinen Clip Ordner aus")
                .set_directory(std::env::current_dir().unwrap_or_default())
                .pick_folder();

            if let Some(folder_path) = result {
                let path_str = folder_path.to_string_lossy().into_owned();
                if let Ok(mut guard) = storage_for_thread.lock() {
                    guard.clip_location = Some(folder_path.clone());
                }
                save_storage(&storage_for_thread);
                let _ = path_tx_for_thread.send_replace(Some(folder_path));
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = ui_weak_for_thread.upgrade() {
                        ui.set_selected_path(path_str.into());
                    }
                });
            }
        });
    });

    let storage_clone_delete_original = Arc::clone(&storage);
    let del_tx_clone = del_tx.clone();
    ui.on_delete_original_changed(move |new_value| {
        let _ = del_tx_clone.send(new_value);
        let storage_for_thread = Arc::clone(&storage_clone_delete_original);
        std::thread::spawn(move || {
            if let Ok(mut guard) = storage_for_thread.lock() {
                guard.delete_original = new_value;
            }
            save_storage(&storage_for_thread);
        });
    });
    ui.set_delete_original(current_delete_original);

    let ui_close_handle = ui_weak.clone();
    ui.window().on_close_requested(move || {
        if let Some(ui) = ui_close_handle.upgrade() {
            ui.hide().expect("Fehler beim Ausblenden des Fensters")
        }

        slint::CloseRequestResponse::KeepWindowShown
    });

    let ui_tray_handle_1 = ui_weak.clone();
    std::thread::spawn(move || {
        let tray_receiver = tray_icon::TrayIconEvent::receiver();
        while let Ok(event) = tray_receiver.recv() {
            if let tray_icon::TrayIconEvent::Click { button, .. } = event {
                if button == tray_icon::MouseButton::Left {
                    let ui_handle = ui_tray_handle_1.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = ui_handle.upgrade() {
                            ui.show().expect("Fehler das Fenster anzuzeigen");
                        }
                    });
                }
            }
        }
    });

    let ui_tray_handle_2 = ui_weak.clone();
    let storage_clone_2 = Arc::clone(&storage);
    std::thread::spawn(move || {
        let menu_receiver = tray_icon::menu::MenuEvent::receiver();
        let ui_handle = ui_tray_handle_2.clone();
        let storage_2_for_thread = Arc::clone(&storage_clone_2);

        while let Ok(event) = menu_receiver.recv() {
            if event.id == quit_item_id {
                save_storage(&storage_2_for_thread);
                let _ = slint::invoke_from_event_loop(|| {
                    slint::quit_event_loop().expect("Fehler den Even Loop zu schließen")
                });
            } else if event.id == open_item_id {
                let ui_handle_clone = ui_handle.clone();

                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = ui_handle_clone.upgrade() {
                        ui.show().expect("Fehler das Fenster anzuzeigen");
                    }
                });
            }
        }
    });

    rt.spawn(async move {
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
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(3 * 60 * 60));

        loop {
            let mut active_path = { path_rx.borrow().clone() };

            while active_path.is_none() {
                println!("Warte auf Pfad");
                if path_rx.changed().await.is_err() {
                    return;
                }
                active_path = path_rx.borrow().clone()
            }
            let clip_folder = active_path.expect("Fehler bei der Ordneränderung");

            if tokio::fs::read_dir(&clip_folder).await.is_err() {
                eprintln!("Ordner nicht gefunden, Pfad zurückgesetzt");
                {
                    let mut guard = storage.lock().expect("Fehler auf AppStorage zuzugreifen");
                    guard.clip_location = None;
                }
                save_storage(&storage);
                let _ = path_tx.send_replace(None);
                let ui_weak_clone = ui_weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = ui_weak_clone.upgrade() {
                        ui.set_selected_path("Kein Pfad ausgewählt".into());
                    }
                });
                continue;
            }

            interval.tick().await;
            println!("Starte Intervall Prüfung...");

            let path = &clip_folder;
            let uploaded_files_clone = {
                let guard = storage.lock().expect("Fehler auf AppStorage zuzugreifen");
                guard.uploaded_files.clone()
            };

            let pending_clips = get_pending_clips(&path, &uploaded_files_clone).await;

            if pending_clips.is_empty() {
                println!("Keine neue Clips gefunden");
                continue;
            }

            println!(
                "Es wurden {} Clips zum hochladen gefunden",
                pending_clips.len()
            );

            for clip_paket in pending_clips.chunks(usize::min(10, pending_clips.len() / 2)) {
                if uploads_today >= 10 {
                    println!("Upload Limit für heute erreicht");
                    break;
                }

                println!("Verarbeite ein Paket von {} Clips...", clip_paket.len());

                let combined_output = PathBuf::from("videos/batch_combined.mp4");

                merge_multiple_videos(clip_paket, &combined_output).await;

                let final_processed_video = process_video_file(&combined_output);

                let video_file = std::fs::File::open(&final_processed_video).expect(&format!(
                    "{:?} konnte nicht geöffnet werden",
                    &final_processed_video
                ));

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
                video_status.privacy_status = Some("unlisted".to_string());
                video.status = Some(video_status);

                let upload_result =
                    upload_video(video, final_processed_video.clone(), &hub, video_file).await;

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
                    save_storage(&storage);
                } else {
                    eprintln!(
                        "Upload fehlgeschlagen. Clips werden nicht als 'hochgeladen' markiert."
                    );
                }

                let _ = tokio::fs::remove_file(combined_output).await;
                let _ = tokio::fs::remove_file(final_processed_video).await;
            }
        }
    });

    ui.show().expect("Fehler das Fenster anzuzeigen");

    slint::run_event_loop_until_quit().expect("Fehler beim Slint Event Loop");
}

fn save_storage(storage: &Arc<Mutex<AppStorage>>) {
    let storage_guard = storage.lock().expect("Fehler beim Storage Guard");
    let ron_string = ron::ser::to_string_pretty(&*storage_guard, ron::ser::PrettyConfig::default())
        .expect("Fehler bei der RON Konvertierung");
    std::fs::write("config.ron", ron_string).expect("Fehler bei der Config Speicherung");
    println!("Gespeichert");
}

fn load_storage() -> Arc<Mutex<AppStorage>> {
    let mut output = AppStorage::default();
    let ron_content = std::fs::read_to_string("config.ron");
    match ron_content {
        Ok(content) => {
            output = ron::from_str(&content).unwrap_or_default();
        }
        _ => {}
    }
    Arc::new(Mutex::new(output))
}

async fn get_pending_clips(path: &Path, uploaded_files: &[String]) -> Vec<PathBuf> {
    use tokio::fs::read_dir;
    let mut pending = Vec::new();

    let mut entries = read_dir(path)
        .await
        .expect("Pfad konnte nicht geöffnet werden");

    while let Some(entry) = entries.next_entry().await.expect("Fehler beim Lesen") {
        let file_path = entry.path();

        if let Some(stem) = file_path.file_stem() {
            if stem.to_string_lossy().ends_with("_converted")
                || stem.to_string_lossy().ends_with("_combined")
            {
                continue;
            }
        }

        if file_path.extension() == Some(&OsStr::new("mp4")) {
            let file_name_str = path_to_string(&file_path);
            if !uploaded_files.contains(&file_name_str) {
                pending.push(file_path);
            }
        }
    }

    pending.sort();
    pending
}

async fn merge_multiple_videos(chunks: &[PathBuf], output_path: &PathBuf) {
    use tokio::io::AsyncWriteExt;

    let (target_w, target_h) = probe_resolution(&chunks[0]).await;
    let mut normalized: Vec<PathBuf> = Vec::with_capacity(chunks.len());

    for (i, clip) in chunks.iter().enumerate() {
        let tmp = PathBuf::from(format!("videos/norm_tmp_{}.mp4", i));
        normalize_clip(clip, &tmp, target_w, target_h).await;
        normalized.push(tmp);
    }

    let mut list_file = tokio::fs::File::create("inputs.txt")
        .await
        .expect("Fehler die inputs.txt zu erstellen");
    for path in &normalized {
        list_file
            .write_all(format!("file '{}'\n", path.to_string_lossy()).as_bytes())
            .await
            .expect("inputs.txt Schreibfehler");
    }
    list_file.flush().await.expect("Flush Fehler");

    let status = tokio::process::Command::new("ffmpeg")
        .arg("-y")
        .args(["-f", "concat", "-safe", "0", "-i", "inputs.txt"])
        .args(["-c", "copy"])
        .arg(output_path)
        .status()
        .await
        .expect("FFmpeg Merge Fehler");

    if status.success() {
        println!("Batch erfolgreich zusammengeführt!");
    } else {
        eprintln!("Merge fehlgeschlagen!");
    }

    let _ = tokio::fs::remove_file("inputs.txt").await;
    for tmp in &normalized {
        let _ = tokio::fs::remove_file(tmp).await;
    }
}

async fn probe_resolution(path: &Path) -> (u32, u32) {
    let output = tokio::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height",
            "-of",
            "csv=p=0",
        ])
        .arg(path)
        .output()
        .await
        .expect("Fehler bei der Auflösung Processing");
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text.lines().next().expect("Fehler bei dem Auflösungstext");
    let mut parts = line.trim().split(",");
    let w: u32 = parts
        .next()
        .expect("Fehler bei der Breite des Videos")
        .parse()
        .expect("Fehler bei der Breite des Videos");
    let h: u32 = parts
        .next()
        .expect("Fehler bei der Höhe des Videos")
        .parse()
        .expect("Fehler bei der Höhe des Videos");
    (w, h)
}

async fn probe_audio_track_count(path: &Path) -> usize {
    if let Ok(output) = tokio::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "a",
            "-show_entries",
            "stream=index",
            "-of",
            "csv=p=0",
        ])
        .arg(path)
        .output()
        .await
    {
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|l| !l.trim().is_empty())
            .count()
    } else {
        0
    }
}

async fn normalize_clip(input: &Path, output: &Path, width: u32, height: u32) {
    let audio_tracks = probe_audio_track_count(input).await;
    println!(
        "Normalisiere {:?}  →  {}×{}  ({} Audiospur/en)",
        input.file_name().unwrap_or_default(),
        width,
        height,
        audio_tracks
    );

    let scale = format!(
        "[0:v]scale={w}:{h}:force_original_aspect_ratio=decrease,\
         pad=w={w}:h={h}:x=(ow-iw)/2:y=(oh-ih)/2:color=black,setsar=1,fps=60[vout]",
        w = width,
        h = height
    );

    let mut cmd = tokio::process::Command::new("ffmpeg");
    cmd.arg("-y").arg("-i").arg(input);

    match audio_tracks {
        0 => {
            cmd.args(["-f", "lavfi", "-i", "anullsrc=r=44100:cl=stereo"]);
            cmd.args(["-filter_complex", &scale])
                .args(["-map", "[vout]", "-map", "1:a:0"])
                .args([
                    "-c:v",
                    "libx264",
                    "-preset",
                    "fast",
                    "-c:a",
                    "aac",
                    "-b:a",
                    "192k",
                    "-ar",
                    "44100",
                    "-shortest",
                ]);
        }
        1 => {
            cmd.args(["-filter_complex", &scale])
                .args(["-map", "[vout]", "-map", "0:a:0"])
                .args([
                    "-c:v",
                    "libx264",
                    "-preset",
                    "fast",
                    "-c:a",
                    "aac",
                    "-b:a",
                    "192k",
                    "-ac",
                    "2",
                    "-ar",
                    "44100",
                    "-shortest",
                ]);
        }
        n => {
            let mix_inputs: String = (0..n).map(|i| format!("[0:a:{}]", i)).collect();
            let filter = format!(
                "{};{}amix=inputs={}:duration=longest[aout]",
                scale, mix_inputs, n
            );
            cmd.args(["-filter_complex", &filter])
                .args(["-map", "[vout]", "-map", "[aout]"])
                .args([
                    "-c:v",
                    "libx264",
                    "-preset",
                    "fast",
                    "-c:a",
                    "aac",
                    "-b:a",
                    "192k",
                    "-ac",
                    "2",
                    "-ar",
                    "44100",
                    "-shortest",
                ]);
        }
    }

    cmd.arg(output);
    let status = cmd.status().await.expect("FFmpeg Normalize Fehler");
    if status.success() {
        println!("✓  {:?}", output.file_name().unwrap_or_default());
    } else {
        eprintln!(
            "✗  Normalisierung fehlgeschlagen: {:?}",
            input.file_name().unwrap_or_default()
        );
    }
}

async fn upload_video(
    video: google_youtube3::api::Video,
    file_path: PathBuf,
    hub: &google_youtube3::YouTube<HttpsConnector<HttpConnector>>,
    video_file: std::fs::File,
) -> Result<(), ()> {
    println!(
        "Starte Upload von der Datei: {:?}:...",
        file_path.file_name()
    );

    let result = hub
        .videos()
        .insert(video)
        .upload(
            video_file,
            "video/*".parse().expect("Fehler beim Video Upload"),
        )
        .await;

    match result {
        Ok((_response, video)) => {
            println!("Video wurde erfolgreich hochgeladen");
            let video_id = video.id.unwrap_or_default();
            println!("Video Link: https://www.youtube.com/watch?v={}", video_id);
            return Ok(());
        }
        Err(error) => {
            eprintln!("Ein Fehler ist aufgetreten: {:?}", error);
            return Err(());
        }
    }
}

fn process_video_file(input_file_path: &PathBuf) -> PathBuf {
    let output_file_path = if let (Some(stem), Some(ext)) =
        (input_file_path.file_stem(), input_file_path.extension())
    {
        let mut name = stem.to_os_string();
        name.push("_converted.");
        name.push(ext);
        input_file_path.with_file_name(name)
    } else {
        input_file_path.clone()
    };

    let status = std::process::Command::new("ffmpeg")
        .arg("-y")
        .arg("-i")
        .arg(input_file_path)
        .args(["-c", "copy", "-movflags", "+faststart"])
        .arg(&output_file_path)
        .status()
        .expect("FFmpeg faststart Fehler");

    if status.success() {
        println!(
            "Videoverarbeitung abgeschlossen: {:?}",
            output_file_path.file_name()
        );
    } else {
        eprintln!("Fehler bei der Videoverarbeitung");
    }
    output_file_path
}

fn path_to_string(file_path: &PathBuf) -> String {
    file_path
        .file_name()
        .map(|os_str| os_str.to_string_lossy().into_owned())
        .unwrap_or_default()
}
