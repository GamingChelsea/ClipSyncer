use serde::Deserialize;
use tracing::info;

#[derive(Debug, Clone)]
pub struct UpdateInfo {
    pub version: String,
    pub download_url: String,
    pub asset_name: String,
    pub release_notes: String,
}

#[derive(Deserialize)]
struct GitHubRelease {
    tag_name: String,
    body: Option<String>,
    assets: Vec<GitHubAsset>,
}

#[derive(Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
}

pub async fn check_for_update() -> Result<Option<UpdateInfo>, Box<dyn std::error::Error + Send + Sync>> {
    let client = reqwest::Client::builder()
        .user_agent("ClipSyncer-AutoUpdater")
        .build()?;

    let url = "https://api.github.com/repos/GamingChelsea/ClipSyncer/releases/latest";
    let resp = client.get(url).send().await?;

    if !resp.status().is_success() {
        return Ok(None);
    }

    let release: GitHubRelease = resp.json().await?;
    let remote_tag = release.tag_name.trim_start_matches('v').trim();

    let current_version_str = env!("CARGO_PKG_VERSION");
    let current_ver = semver::Version::parse(current_version_str)?;

    let remote_ver = match semver::Version::parse(remote_tag) {
        Ok(v) => v,
        Err(_) => return Ok(None),
    };

    if remote_ver <= current_ver {
        info!("App ist aktuell (Lokal: {}, Remote: {})", current_ver, remote_ver);
        return Ok(None);
    }

    info!("Neues Update gefunden! (Lokal: {}, Remote: {})", current_ver, remote_ver);

    // Finde das passende Asset für das aktuelle Betriebssystem
    let target_extension = if cfg!(target_os = "windows") {
        ".msi"
    } else if cfg!(target_os = "linux") {
        "linux-x86_64.tar.gz"
    } else if cfg!(target_os = "macos") {
        "macos-aarch64.tar.gz"
    } else {
        ""
    };

    let asset = release.assets.into_iter().find(|a| {
        if target_extension.is_empty() {
            false
        } else {
            a.name.ends_with(target_extension)
        }
    });

    if let Some(asset) = asset {
        Ok(Some(UpdateInfo {
            version: remote_tag.to_string(),
            download_url: asset.browser_download_url,
            asset_name: asset.name,
            release_notes: release.body.unwrap_or_default(),
        }))
    } else {
        Ok(None)
    }
}

pub async fn download_and_install_update<F>(
    download_url: &str,
    asset_name: &str,
    progress_callback: F,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
where
    F: Fn(f32) + Send + Sync + 'static,
{
    let client = reqwest::Client::builder()
        .user_agent("ClipSyncer-AutoUpdater")
        .build()?;

    info!("Lade Update herunter von: {}", download_url);
    let resp = client.get(download_url).send().await?;

    let total_size = resp.content_length().unwrap_or(0);
    let mut downloaded: u64 = 0;

    let temp_file_path = std::env::temp_dir().join(asset_name);
    let mut file = tokio::fs::File::create(&temp_file_path).await?;

    use tokio::io::AsyncWriteExt;

    let mut resp = resp;
    while let Some(chunk) = resp.chunk().await? {
        file.write_all(&chunk).await?;
        downloaded += chunk.len() as u64;

        if total_size > 0 {
            let progress = (downloaded as f32) / (total_size as f32);
            progress_callback(progress);
        }
    }

    file.flush().await?;
    info!("Update-Download abgeschlossen: {:?}", temp_file_path);

    #[cfg(windows)]
    {
        if asset_name.ends_with(".msi") {
            info!("Starte MSI-Installer für automatisches In-Place Update...");
            // Führe msiexec aus
            let _ = std::process::Command::new("msiexec.exe")
                .arg("/i")
                .arg(&temp_file_path)
                .arg("/passive")
                .spawn();

            // Beende die aktuelle Anwendung, damit MSI die Dateien ersetzen kann
            info!("Beende ClipSyncer für Installer...");
            std::process::exit(0);
        }
    }

    Ok(())
}
