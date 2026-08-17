use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{error, info};

use crate::storage::VideoEncoder;

fn create_command(program: &str) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new(program);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    cmd
}

pub async fn get_pending_clips(path: &Path, uploaded_files: &[String]) -> Vec<PathBuf> {
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

        if file_path.extension() == Some(OsStr::new("mp4")) {
            let file_name_str = path_to_string(&file_path);
            if !uploaded_files.contains(&file_name_str) {
                pending.push(file_path);
            }
        }
    }

    pending.sort();
    pending
}

pub async fn merge_multiple_videos(
    chunks: &[PathBuf],
    output_path: &Path,
    video_encoder: VideoEncoder,
) -> bool {
    use tokio::io::AsyncWriteExt;

    let (target_w, target_h) = match probe_resolution(&chunks[0]).await {
        Some(res) => res,
        None => {
            error!("Konnte Auflösung des ersten Clips nicht bestimmen");
            return false;
        }
    };

    let semaphore = Arc::new(tokio::sync::Semaphore::new(3));
    let mut workers = tokio::task::JoinSet::new();

    for (index, clip) in chunks.iter().enumerate() {
        let clip = clip.clone();
        let sem = Arc::clone(&semaphore);

        workers.spawn(async move {
            let _permit = sem.acquire().await.expect("Semaphore Fehler");

            let tmp_file = tempfile::Builder::new()
                .prefix("norm_tmp_")
                .suffix(".mp4")
                .tempfile()?;

            if normalize_clip(&clip, tmp_file.path(), target_w, target_h, video_encoder).await {
                Ok((index, tmp_file))
            } else {
                Err(std::io::Error::other("Normalisierung fehlgeschlagen"))
            }
        });
    }

    let mut completed_result = Vec::with_capacity(chunks.len());

    while let Some(res) = workers.join_next().await {
        match res {
            Ok(Ok((index, tmp_file))) => completed_result.push((index, tmp_file)),
            _ => {
                error!("Ein Clip Task ist fehlgeschlagen. Breche Paketverarbeitung ab");
                return false;
            }
        }
    }

    completed_result.sort_by_key(|(index, _)| *index);
    let normalized_temp_file: Vec<_> = completed_result.into_iter().map(|(_, tmp)| tmp).collect();

    let list_file_temp = match tempfile::Builder::new()
        .prefix("inputs_")
        .suffix(".txt")
        .tempfile()
    {
        Ok(f) => f,
        Err(e) => {
            error!(error = ?e, "Fehler beim Erstellen der temporären inputs-Datei:");
            return false;
        }
    };

    let mut list_file = match tokio::fs::File::create(list_file_temp.path()).await {
        Ok(f) => f,
        Err(e) => {
            error!(error = ?e, "Fehler beim Erstellen von inputs.txt per Tokio");
            return false;
        }
    };

    for tmp_file in &normalized_temp_file {
        let path_str = tmp_file.path().to_string_lossy().replace('\\', "/");
        let line = format!("file '{}'\n", path_str);

        if let Err(e) = list_file.write_all(line.as_bytes()).await {
            error!(error = ?e, "Fehler beim Schreiben in die Conact List");
            return false;
        }
    }

    if let Err(e) = list_file.flush().await {
        error!(error = ?e, "Fehler beim Flushen der Conact Liste:");
        return false;
    }

    let status = create_command("ffmpeg")
        .arg("-y")
        .args(["-loglevel", "error"])
        .args(["-f", "concat", "-safe", "0", "-i"])
        .arg(list_file_temp.path())
        .args(["-c", "copy"])
        .arg(output_path)
        .stdout(std::process::Stdio::null())
        .status()
        .await;

    match status {
        Ok(s) if s.success() => {
            info!("Batch erfolgreich zusammengeführt");
            true
        }
        _ => {
            error!("Merge mit FFmpeg fehlgeschlagen");
            false
        }
    }
}

pub async fn probe_resolution(path: &Path) -> Option<(u32, u32)> {
    let output = create_command("ffprobe")
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
        .await;

    let output = match output {
        Ok(out) => out,
        Err(e) => {
            error!(error = ?e, "Fehler beim Ausführen von ffprobe");
            return None;
        }
    };

    let text = String::from_utf8_lossy(&output.stdout);
    let line = match text.lines().next() {
        Some(l) => l,
        None => {
            error!("ffprobe hat keine Ausgabe geliefert (Datei evtl. beschädigt)");
            return None;
        }
    };
    let mut parts = line.trim().split(",");
    let w: u32 = parts.next()?.trim().parse().ok()?;
    let h: u32 = parts.next()?.trim().parse().ok()?;

    Some((w, h))
}

pub async fn probe_audio_track_count(path: &Path) -> usize {
    if let Ok(output) = create_command("ffprobe")
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

pub async fn normalize_clip(
    input: &Path,
    output: &Path,
    width: u32,
    height: u32,
    video_encoder: VideoEncoder,
) -> bool {
    let audio_tracks = probe_audio_track_count(input).await;
    info!(
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

    let selected_encoder = match video_encoder {
        VideoEncoder::Auto => detect_best_encoder().await,
        other => other,
    };

    let encoder_str = match selected_encoder {
        VideoEncoder::Nvidia => "h264_nvenc",
        VideoEncoder::Amd => "h264_amf",
        VideoEncoder::Intel => "h264_qsv",
        _ => "libx264",
    };

    let mut cmd = create_command("ffmpeg");
    cmd.arg("-y")
        .args(["-loglevel", "error"])
        .arg("-i")
        .arg(input)
        .stdout(std::process::Stdio::null());

    match audio_tracks {
        0 => {
            cmd.args(["-f", "lavfi", "-i", "anullsrc=r=44100:cl=stereo"]);
            cmd.args(["-filter_complex", &scale])
                .args(["-map", "[vout]", "-map", "1:a:0"])
                .args([
                    "-c:v",
                    encoder_str,
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
                    encoder_str,
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
                    encoder_str,
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
    match cmd.status().await {
        Ok(status) if status.success() => {
            info!("✓  {:?}", output.file_name().unwrap_or_default());
            true
        }
        _ => {
            error!(
                filename = ?input.file_name().unwrap_or_default(),
                "✗  Normalisierung fehlgeschlagen:");
            false
        }
    }
}

pub async fn process_video_file(input_file_path: &Path, output_file_path: &Path) -> bool {
    let status = create_command("ffmpeg")
        .arg("-y")
        .args(["-loglevel", "error"])
        .args(["-i"])
        .arg(input_file_path)
        .args(["-c", "copy", "-movflags", "+faststart"])
        .arg(output_file_path)
        .stdout(std::process::Stdio::null())
        .status()
        .await;

    match status {
        Ok(s) if s.success() => {
            info!(
                "Videoverarbeitung abgeschlossen: {:?}",
                output_file_path.file_name()
            );
            true
        }
        _ => {
            error!("Fehler bei der Videoverarbeitung (faststart)");
            false
        }
    }
}

pub fn path_to_string(file_path: &Path) -> String {
    file_path
        .file_name()
        .map(|os_str| os_str.to_string_lossy().into_owned())
        .unwrap_or_default()
}

pub async fn detect_best_encoder() -> VideoEncoder {
    let output = create_command("ffmpeg")
        .arg("-encoders")
        .output()
        .await;

    if let Ok(out) = output {
        let text = String::from_utf8_lossy(&out.stdout);
        if text.contains("h264_nvenc") {
            return VideoEncoder::Nvidia;
        } else if text.contains("h264_amf") {
            return VideoEncoder::Amd;
        } else if text.contains("h264_qsv") {
            return VideoEncoder::Intel;
        }
    };
    VideoEncoder::Cpu
}
