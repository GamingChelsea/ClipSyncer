use bytes::Bytes;
use google_youtube3::hyper_rustls::HttpsConnector;
use google_youtube3::hyper_util;
use google_youtube3::hyper_util::client::legacy::connect::HttpConnector;
use http_body_util::combinators::BoxBody;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use yup_oauth2::InstalledFlowAuthenticator;

slint::include_modules!();

type GoogleClient = hyper_util::client::legacy::Client<
    HttpsConnector<HttpConnector>,
    BoxBody<Bytes, google_youtube3::hyper::Error>,
>;

fn main() {
    let rt = tokio::runtime::Runtime::new().expect("Tokio Runtime Fehler");

    let image_path = "assets/icon.png";
    let dynamic_image = image::open(image_path).expect("Fehler beim öffnen vom Icon");
    let rgba_image = dynamic_image.to_rgba8();
    let (width, height) = rgba_image.dimensions();
    let raw_pixels = rgba_image.into_raw();

    let icon =
        tray_icon::Icon::from_rgba(raw_pixels, width, height).expect("Fehler beim Icon Tray");

    let tray_menu = tray_icon::menu::Menu::new();
    let quit_item = tray_icon::menu::MenuItem::new("Beenden", true, None);
    let quit_item_id = quit_item.id().clone();

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

    let ui_close_handle = ui_weak.clone();
    ui.window().on_close_requested(move || {
        if let Some(ui) = ui_close_handle.upgrade() {
            ui.hide().unwrap()
        }

        slint::CloseRequestResponse::KeepWindowShown
    });

    let ui_tray_handle = ui_weak.clone();
    std::thread::spawn(move || {
        let tray_receiver = tray_icon::TrayIconEvent::receiver();
        while let Ok(event) = tray_receiver.recv() {
            if let tray_icon::TrayIconEvent::Click { button, .. } = event {
                if button == tray_icon::MouseButton::Left {
                    let ui_handle = ui_tray_handle.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = ui_handle.upgrade() {
                            ui.show().unwrap();
                        }
                    });
                }
            }
        }
    });

    std::thread::spawn(move || {
        let menu_receiver = tray_icon::menu::MenuEvent::receiver();
        while let Ok(event) = menu_receiver.recv() {
            if event.id == quit_item_id {
                let _ = slint::invoke_from_event_loop(|| slint::quit_event_loop().unwrap());
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
        .unwrap();

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
        let path = Path::new(".").join("videos");

        let mut binding = tokio::fs::OpenOptions::new();

        let mut uploaded_list_file = binding
            .create(true)
            .append(true)
            .read(true)
            .open("uploaded_files.txt")
            .await
            .expect("Fehler beim lesen/erstellen der uploaded_files.txt Datei");

        let reader = BufReader::new(&mut uploaded_list_file);
        let mut lines = reader.lines();
        let mut uploaded_files = Vec::<String>::new();
        while let Some(line) = lines
            .next_line()
            .await
            .expect("Fehler beim Lesen der Zeile")
        {
            uploaded_files.push(line);
        }

        loop {
            scan_dir(
                path.clone(),
                hub.clone(),
                &mut uploaded_files,
                &mut uploaded_list_file,
            )
            .await;

            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    });

    ui.show().unwrap();

    slint::run_event_loop_until_quit().expect("Fehler beim Slint Event Loop");
}

async fn scan_dir(
    path: PathBuf,
    hub: google_youtube3::YouTube<HttpsConnector<HttpConnector>>,
    uploaded_files: &mut Vec<String>,
    uploaded_list_file: &mut tokio::fs::File,
) {
    if false {
        use tokio::fs::read_dir;

        let mut entries = read_dir(&path)
            .await
            .expect(&format!("Pfad {:?} konnte nicht geöffnet werden", &path));

        while let Some(entry) = entries
            .next_entry()
            .await
            .expect(&format!("Pfad {:?} konnte nicht gelesen werden", &path))
        {
            let file_path = entry.path();
            if let Some(stem) = file_path.file_stem() {
                if stem.to_string_lossy().ends_with("_converted") {
                    continue;
                }
            }

            if file_path.extension() == Some(OsStr::new("mp4"))
                && !uploaded_files.contains(&path_to_string(&file_path))
            {
                let converted_path = process_video_file(&file_path);

                let video_file = std::fs::File::open(&converted_path).expect(&format!(
                    "{:?} konnte nicht geöffnet werden",
                    &converted_path
                ));

                let mut video = google_youtube3::api::Video::default();

                let mut details = google_youtube3::api::VideoSnippet::default();
                details.title = Some(
                    file_path
                        .file_name()
                        .and_then(|os_str| os_str.to_str())
                        .map(|s| s.to_string())
                        .unwrap_or_default(),
                );
                details.category_id = Some("22".to_string());
                video.snippet = Some(details);

                let mut video_status = google_youtube3::api::VideoStatus::default();
                video_status.privacy_status = Some("unlisted".to_string());
                video.status = Some(video_status);

                upload_video(
                    video,
                    file_path.clone(),
                    &hub,
                    video_file,
                    uploaded_files,
                    uploaded_list_file,
                )
                .await;

                let _ = tokio::fs::remove_file(converted_path).await;
            }
        }
    }
}

fn process_video_file(input_file_path: &PathBuf) -> PathBuf {
    let output_file_path = if let (Some(stem), Some(ext)) =
        (input_file_path.file_stem(), input_file_path.extension())
    {
        let mut new_filename = stem.to_os_string();
        new_filename.push("_converted.");
        new_filename.push(ext);
        input_file_path.with_file_name(new_filename)
    } else {
        input_file_path.clone()
    };

    let mut audio_track_count = 0;

    if let Ok(output) = std::process::Command::new("ffprobe")
        .arg("-v")
        .arg("error")
        .arg("-select_streams")
        .arg("a")
        .arg("-show_entries")
        .arg("stream=index")
        .arg("-of")
        .arg("csv=p=0")
        .arg(input_file_path)
        .output()
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        audio_track_count = stdout.lines().filter(|l| !l.trim().is_empty()).count();
    }

    println!(
        "Datei {:?} hat {} Audiospur(en).",
        input_file_path.file_name().unwrap_or_default(),
        audio_track_count
    );

    let mut command = ffmpeg_sidecar::command::FfmpegCommand::new();
    command.arg("-y");
    command.arg("-i").arg(input_file_path);

    if audio_track_count == 0 {
        println!("Video ist stumm. Kopiere nur die Videospur...");
        command
            .arg("-c:v")
            .arg("copy")
            .arg("-an")
            .output(output_file_path.to_str().unwrap());
    } else if audio_track_count == 1 {
        println!("Nur 1 Audiospur gefunden. Mischen nicht notwendig.");
        command
            .arg("-c:v")
            .arg("copy")
            .arg("-c:a")
            .arg("aac")
            .arg("-b:a")
            .arg("320k")
            .arg("-movflags")
            .arg("+faststart")
            .output(output_file_path.to_str().unwrap());
    } else {
        println!("Mehrere Spuren gefunden. Starte Audio-Mix...");
        let mut filter_input = String::new();
        for i in 0..audio_track_count {
            filter_input.push_str(&format!("[0:a:{}]", i));
        }
        let filter_complex_str = format!(
            "{}amix=inputs={}:duration=longest[aout]",
            filter_input, audio_track_count
        );

        command
            .arg("-filter_complex")
            .arg(&filter_complex_str)
            .arg("-map")
            .arg("0:v:0")
            .arg("-map")
            .arg("[aout]")
            .arg("-c:v")
            .arg("copy")
            .arg("-c:a")
            .arg("aac")
            .arg("-b:a")
            .arg("320k")
            .output(output_file_path.to_str().unwrap());
    }

    let mut child = command.spawn().expect("ffmpeg Fehler beim Starten");

    for event in child.iter().expect("Fehler beim Iterieren vom FFmpeg") {
        match event {
            ffmpeg_sidecar::event::FfmpegEvent::Progress(progress) => {
                println!(
                    "Fortschritt: Frame {}, Zeit: {}s, Geschwindigkeit: {}x",
                    progress.frame, progress.time, progress.speed
                );
            }
            ffmpeg_sidecar::event::FfmpegEvent::Log(level, msg) => {
                if level == ffmpeg_sidecar::event::LogLevel::Error {
                    eprintln!("FFmpeg Fehler: {}", msg);
                }
            }
            _ => {}
        }
    }

    let _ = child.wait();

    println!("Videoverarbeitung abgeschlossen.");
    output_file_path
}

async fn upload_video(
    video: google_youtube3::api::Video,
    file_path: PathBuf,
    hub: &google_youtube3::YouTube<HttpsConnector<HttpConnector>>,
    video_file: std::fs::File,
    uploaded_files: &mut Vec<String>,
    uploaded_list_file: &mut tokio::fs::File,
) {
    println!(
        "Starte Upload von der Datei: {:?}: ...",
        file_path.file_name()
    );

    if false {
        let result = hub
            .videos()
            .insert(video)
            .upload(video_file, "video/*".parse().unwrap())
            .await;

        match result {
            Ok((_response, video)) => {
                println!("Video wurde erfolgreich hochgeladen");
                let video_id = video.id.unwrap_or_default();
                println!("Video Link: https://www.youtube.com/watch?v={}", video_id);
                let new_entry = path_to_string(&file_path);
                let data_formatted = format!("\n{}", &new_entry);
                uploaded_files.push(new_entry);
                uploaded_list_file
                    .write_all(data_formatted.as_bytes())
                    .await
                    .unwrap();
            }
            Err(error) => {
                eprintln!("Ein Fehler ist aufgetreten: {:?}", error);
            }
        }
    }
}

fn path_to_string(file_path: &PathBuf) -> String {
    file_path
        .file_name()
        .map(|os_str| os_str.to_string_lossy().into_owned())
        .unwrap_or_default()
}
