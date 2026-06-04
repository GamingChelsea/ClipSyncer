use ffmpeg_sidecar::{command::FfmpegCommand, ffmpeg_with_args};
use std::process::Stdio;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let input_file = "input.mkv";
    let output_file = "output.mp4";

    // 1. (Optional) Anzahl der Audiospuren dynamisch ermitteln
    // Für dieses Beispiel nehmen wir an, das Video hat 3 Spuren (z.B. Game, Discord, Mic)
    let audio_track_count = 3;

    // 2. Filter-String dynamisch bauen
    // Erzeugt: "[0:a:0][0:a:1][0:a:2]amix=inputs=3:duration=longest[aout]"
    let mut filter_input = String::new();
    for i in 0..audio_track_count {
        filter_input.push_str(&format!("[0:a:{}]", i));
    }
    let filter_complex_str = format!(
        "{}amix=inputs={}:duration=longest[aout]",
        filter_input, audio_track_count
    );

    // 3. Den FFmpeg-Befehl typsicher in Rust aufbauen
    let mut command = FfmpegCommand::new();

    command
        .input(input_file)
        .arg("-filter_complex")
        .arg(&filter_complex_str)
        .arg("-map")
        .arg("0:v:0") // Erste Videospur auswählen
        .arg("-map")
        .arg("[aout]") // Das gemischte Audio-Ergebnis auswählen
        .arg("-c:v")
        .arg("copy") // Video OHNE Rendering kopieren (Stream Copy)
        .arg("-c:a")
        .arg("aac") // Audio in YouTube-freundliches AAC konvertieren
        .arg("-b:a")
        .arg("320k") // Hohe Audio-Bitrate für YouTube
        .output(output_file);

    // 4. Prozess starten und Fortschritt in Echtzeit überwachen
    let mut child = command.spawn()?;

    // Iteriert über die FFmpeg-Log-Ausgaben (Fortschritt, FPS, Zeit)
    for event in child.iter() {
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

    println!("Datei erfolgreich für YouTube verarbeitet!");
    Ok(())
}
