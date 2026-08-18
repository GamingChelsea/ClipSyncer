# ClipSyncer

ClipSyncer is a lightweight desktop utility built in Rust that monitors a folder for new gameplay clips, processes and normalizes them with FFmpeg, and automatically uploads them to your YouTube channel using the YouTube Data API v3.

---

## What it does

- **Folder Watcher:** Watches your recording folder (e.g. NVIDIA ShadowPlay, OBS, Medal) and detects new `.mp4` recordings.
- **Audio Mixing & Processing:** Automatically mixes separate audio tracks (e.g. Game Sound, Discord, Microphone) into a single clean stereo output.
- **Hardware Acceleration:** Uses GPU encoders (`h264_nvenc` for NVIDIA, `h264_amf` for AMD, `h264_qsv` for Intel) or CPU (`libx264`) with `+faststart` web optimization.
- **YouTube Uploads:** Uploads videos directly to YouTube with configurable privacy (*Private*, *Unlisted*, or *Public*) and multi-account backup switching.
- **Silent Background Operation:** Runs in the system tray, launches with Windows if desired, and executes FFmpeg without stealing window focus during games.
- **Embedded FFmpeg:** Automatically fetches local FFmpeg binaries on first launch if not already installed.

---

## Installation & Downloads

Pre-built binaries for Windows, Linux, and macOS are available under [**Releases**](https://github.com/GamingChelsea/ClipSyncer/releases):

- **Windows Installer (`.msi`):** Recommended for daily use. Includes start menu shortcuts and in-place upgrade support.
- **Windows Portable (`.zip`):** Standalone executable with static CRT. Run anywhere without installation.
- **Linux (`.tar.gz`):** x86_64 binary.
- **macOS (`.tar.gz`):** Apple Silicon (ARM64) binary.

---

## Google API Setup (100% Free)

To upload videos to your YouTube channel, you need your own Google OAuth client credentials (`client_secret.json`). The YouTube Data API provides a free daily quota of 10,000 units (approx. 6 video uploads per day) without requiring billing or a credit card.

### Step 1: Create a Google Cloud Project
1. Log in to the [Google Cloud Console](https://console.cloud.google.com/).
2. Click the project dropdown in the top-left and select **New Project**.
3. Name it (e.g., `ClipSyncer`) and click **Create**.

### Step 2: Enable the YouTube Data API
1. Open **APIs & Services** → **Library**.
2. Search for **`YouTube Data API v3`**.
3. Select it and click **Enable**.

### Step 3: Configure the OAuth Consent Screen
1. Go to **APIs & Services** → **OAuth consent screen**.
2. Select **External** and click **Create**.
3. Fill in the required fields:
   - **App name:** `ClipSyncer`
   - **User support email:** Your email address
   - **Developer contact email:** Your email address
4. Click **Save and Continue**.
5. Under **Scopes**, click **Add or Remove Scopes**, select `.../auth/youtube.upload`, and click **Update** → **Save and Continue**.
6. Under **Test Users** (*Important*):
   - Click **+ Add Users**.
   - Enter the Google account email address you plan to upload videos to.
   - Click **Save and Continue**.

### Step 4: Generate `client_secret.json`
1. Go to **APIs & Services** → **Credentials**.
2. Click **+ Create Credentials** → **OAuth client ID**.
3. Set Application type to **Desktop App**.
4. Name it (e.g., `ClipSyncer Client`) and click **Create**.
5. Click **Download JSON** on the confirmation dialog.

---

## First Run & Configuration

1. Launch **ClipSyncer**.
2. On first launch, the **Google API Setup** prompt will ask for your `client_secret.json`. Click **Select file** and choose the downloaded JSON.
3. Your default browser will open with the Google OAuth consent page. Sign in with your test user Google account and approve permissions.
4. In the app settings, select your local clips folder.
5. ClipSyncer will now monitor the directory and automatically queue, process, and upload new clips.

---

## Building from Source

Ensure you have Rust (2024 edition / stable) installed:

```bash
# Clone the repository
git clone https://github.com/GamingChelsea/ClipSyncer.git
cd ClipSyncer

# Run in debug mode
cargo run

# Build optimized release binary
cargo build --release
```

---

## AI Notice

This repository uses AI assistance for code refactoring, platform compatibility fixes, installer packaging scripts, and documentation improvements. Commits assisted by AI are prefixed with `#AI` in the commit history for transparency.

---

## License

This project is licensed under the [MIT License](LICENSE).
