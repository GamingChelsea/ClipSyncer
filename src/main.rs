use bytes::Bytes;
use google_youtube3::hyper_rustls::HttpsConnector;
use google_youtube3::hyper_util;
use google_youtube3::hyper_util::client::legacy::connect::HttpConnector;
use http_body_util::combinators::BoxBody;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tray_icon::TrayIconEvent;
use yup_oauth2::InstalledFlowAuthenticator;

type GoogleClient = hyper_util::client::legacy::Client<
    HttpsConnector<HttpConnector>,
    BoxBody<Bytes, google_youtube3::hyper::Error>,
>;

struct ClipSyncerApp {
    tray_receiver: &'static crossbeam_channel::Receiver<TrayIconEvent>,
    window: Option<winit::window::Window>,
}

impl winit::application::ApplicationHandler for ClipSyncerApp {
    fn resumed(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        event_loop.set_control_flow(winit::event_loop::ControlFlow::Wait);
    }
    fn about_to_wait(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        if let Ok(event) = self.tray_receiver.try_recv() {
            println!("Tray Icon Event empfangen: {:?}", event);

            match event {
                TrayIconEvent::DoubleClick { .. } => {
                    if self.window.is_none() {
                        println!("Öffne Fenster");

                        let window_attributes = winit::window::Window::default_attributes()
                            .with_title("Clip Syncer Dashboard");

                        let win = event_loop
                            .create_window(window_attributes)
                            .expect("Fehler bei der Fenstererstellung");
                        self.window = Some(win)
                    } else {
                        println!("Fenster ist schon offen");
                    }
                }
                _ => {}
            }
            // GUI FENSTER ÖFFNEN SCHLIEẞEN
        }
    }
    fn window_event(
        &mut self,
        _event_loop: &winit::event_loop::ActiveEventLoop,
        window_id: winit::window::WindowId,
        event: winit::event::WindowEvent,
    ) {
        match event {
            winit::event::WindowEvent::CloseRequested => {
                if let Some(window) = &self.window {
                    if window.id() == window_id {
                        println!("Fenster geschlossen, App läuft im Tray weiter");
                        self.window = None
                    }
                }
            }
            _ => (),
        }
    }
}

fn main() {
    let rt = tokio::runtime::Runtime::new().expect("Tokio Runtime Fehler");

    let tray_menu = tray_icon::menu::Menu::new();
    let _tray_icon = tray_icon::TrayIconBuilder::new()
        .with_tooltip("Clip Syncer")
        .with_menu(Box::new(tray_menu))
        .build()
        .expect("Fehler beim erstellen des Tray Icons");
    let tray_receiver = tray_icon::TrayIconEvent::receiver();

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

    let event_loop = winit::event_loop::EventLoop::new().expect("Event Loop Erstellungs-Fehler");
    let mut app = ClipSyncerApp {
        tray_receiver,
        window: None,
    };

    event_loop
        .run_app(&mut app)
        .expect("Fehler beim Ausführen der App");
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
