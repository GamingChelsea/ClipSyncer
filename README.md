# 🎬 ClipSyncer

<div align="center">

**Automatische Synchronisierung, Normalisierung und Uploads deiner Gaming-Clips auf YouTube.**

[![Release Build](https://github.com/GamingChelsea/ClipSyncer/actions/workflows/release.yml/badge.svg)](https://github.com/GamingChelsea/ClipSyncer/actions/workflows/release.yml)
[![GitHub Release](https://img.shields.io/github/v/release/GamingChelsea/ClipSyncer?color=blue)](https://github.com/GamingChelsea/ClipSyncer/releases)
[![License: MIT](https://img.shields.io/badge/License-MIT-green.svg)](LICENSE)

[Features](#-features) • [Installation](#-installation) • [Google API Setup (Kostenlos)](#-google-api-setup-schritt-f%C3%BCr-schritt) • [Erster Start](#-erster-start) • [FAQ](#-faq--troubleshooting)

</div>

---

## ✨ Features

- 📁 **Automatischer Ordner-Watcher:** Überwacht deinen Clip-Ordner im Hintergrund und verarbeitet neue Videos automatisch.
- ⚡ **Hardwarebeschleunigung & Faststart:** Unterstützt **NVIDIA (NVENC)**, **AMD (AMF)**, **Intel (QSV)** und CPU-Encoding mit automatischem `+faststart`-Processing.
- 🎚️ **Smart Multi-Audio Mixing:** Mischt mehrere Audiospuren (z. B. Spiel-Sound, Discord, Mikrofon) automatisch via FFmpeg sauber zusammen.
- 🔒 **YouTube Upload & Multi-Account:** Lädt Clips automatisch als *Privat*, *Nicht gelistet* oder *Öffentlich* hoch – mit Unterstützung für Haupt- und Backup-Accounts.
- 📦 **Zero-Config FFmpeg:** Lädt FFmpeg beim ersten Start auf Wunsch automatisch im Hintergrund herunter.
- 🪟 **System-Tray Integration & Autostart:** Läuft dezent im Hintergrund, startet optional mit Windows und stiehlt beim Encodieren niemals den Spielfokus.
- 🌍 **Mehrsprachig:** Integrierte Sprachauswahl (Deutsch / English).

---

## 📥 Installation

Lade die passende Version direkt von der [Releases-Seite](https://github.com/GamingChelsea/ClipSyncer/releases) herunter:

| Plattform | Dateiformat | Beschreibung |
| :--- | :--- | :--- |
| **Windows** | `ClipSyncer-x.x.x-x86_64.msi` | Komfortabler Windows-Installer mit Startmenü- und Autostart-Option |
| **Windows (Portable)** | `ClipSyncer-x.x.x-windows-x86_64.zip` | Standalone `.exe` ohne Installation (direkt startbar) |
| **Linux** | `ClipSyncer-x.x.x-linux-x86_64.tar.gz` | Linux x86_64 Binary |
| **macOS** | `ClipSyncer-x.x.x-macos-aarch64.tar.gz` | macOS Binary (Apple Silicon M1/M2/M3/M4) |

---

## 🔑 Google API Setup (Schritt für Schritt)

> [!NOTE]
> Die Nutzung der YouTube Data API v3 über Google Cloud ist **100% kostenlos**. Es fallen keine Gebühren an und es ist keine Kreditkarte erforderlich.

### Schritt 1: Projekt in der Google Cloud Console anlegen
1. Öffne die **[Google Cloud Console](https://console.cloud.google.com/)** und melde dich mit deinem Google-Konto an.
2. Klicke oben links auf das Projektauswahl-Menü und wähle **"Neues Projekt"** (*New Project*).
3. Vergib einen beliebigen Projektnamen (z. B. `ClipSyncer`) und klicke auf **"Erstellen"** (*Create*).

### Schritt 2: YouTube Data API v3 aktivieren
1. Gehe im linken Menü auf **"APIs und Dienste"** (*APIs & Services*) ➔ **"Bibliothek"** (*Library*).
2. Suche nach **`YouTube Data API v3`**.
3. Klicke auf die API und wähle **"Aktivieren"** (*Enable*).

### Schritt 3: OAuth-Zustimmungsbildschirm einrichten
1. Gehe im linken Menü auf **"OAuth-Zustimmungsbildschirm"** (*OAuth consent screen*).
2. Wähle als Nutzertyp **"Extern"** (*External*) und klicke auf **"Erstellen"**.
3. Fülle die Pflichtfelder aus:
   - **App-Name:** `ClipSyncer`
   - **E-Mail-Adresse für Nutzersupport:** Deine eigene E-Mail-Adresse
   - **E-Mail-Adresse des Entwicklers:** Deine eigene E-Mail-Adresse
4. Klicke auf **"Speichern und fortfahren"** (*Save and Continue*).
5. Bei **Bereiche (Scopes)**:
   - Klicke auf **"Bereiche hinzufügen oder entfernen"** (*Add or remove scopes*).
   - Suche nach `.../auth/youtube.upload` und setze das Häkchen.
   - Klicke auf **"Aktualisieren"** und danach auf **"Speichern und fortfahren"**.
6. Bei **Testnutzer (Test users)** (WICHTIG):
   - Klicke auf **"+ Add Users"** (*Nutzer hinzufügen*).
   - Trage die Google/YouTube-E-Mail-Adresse ein, mit der du Videos hochladen möchtest.
   - Klicke auf **"Speichern und fortfahren"**.

### Schritt 4: OAuth-Client-ID erstellen & `client_secret.json` herunterladen
1. Gehe im linken Menü auf **"Anmeldedaten"** (*Credentials*).
2. Klicke oben auf **"+ Anmeldedaten erstellen"** ➔ **"OAuth-Client-ID"**.
3. Wähle als Anwendungstyp **"Desktop-App"** (*Desktop App*).
4. Vergib einen Namen (z. B. `ClipSyncer Client`) und klicke auf **"Erstellen"**.
5. Klicke im erscheinenden Fenster auf **"JSON herunterladen"** (*Download JSON*).

---

## 🚀 Erster Start

1. Starte **ClipSyncer**.
2. **Google API Setup:** Beim ersten Start erscheint automatisch der Setup-Dialog. Klicke auf **"Datei auswählen"** und wähle deine soeben heruntergeladene JSON-Datei (`client_secret.json`) aus.
3. **Google Login:** Dein Browser öffnet sich automatisch. Melde dich mit deinem Testnutzer-Google-Konto an und erlaube den Upload-Zugriff. *(Hinweis: Google zeigt eventuell „Diese App wurde nicht überprüft“ – klicke auf „Erweitert“ ➔ „Weiter zu ClipSyncer“).*
4. **Ordner wählen:** Wähle in den Einstellungen deinen lokalen Clip-Ordner aus (z. B. NVIDIA ShadowPlay / OBS Aufnahme-Ordner).
5. **Fertig!** ClipSyncer überwacht den Ordner nun automatisch im Hintergrund.

---

## ❓ FAQ & Troubleshooting

<details>
<summary><b>Kostet die YouTube API etwas?</b></summary>
Nein. Google stellt jedem Projekt täglich ein kostenloses Kontingent von 10.000 Einheiten zur Verfügung. Ein Video-Upload verbraucht ca. 1.600 Einheiten, sodass du täglich ca. 6 Videos komplett kostenlos hochladen kannst.
</details>

<details>
<summary><b>Warum muss ich mich als "Testnutzer" eintragen?</b></summary>
Weil die App im Google "Testing"-Modus läuft. Dadurch sparst du dir die monatelange, aufwändige und teure Verifizierung von Google und kannst deine eigene App sofort privat nutzen.
</details>

<details>
<summary><b>Muss FFmpeg separat installiert werden?</b></summary>
Nein. ClipSyncer erkennt automatisch, ob FFmpeg auf deinem System installiert ist. Falls nicht, lädt die App FFmpeg beim ersten Start automatisch im Hintergrund herunter.
</details>

---

## 🛠️ Entwicklung

```bash
# Repository klonen
git clone https://github.com/GamingChelsea/ClipSyncer.git
cd ClipSyncer

# Debug-Build starten
cargo run

# Release-Build erstellen
cargo build --release
```

---

## 📄 Lizenz

Dieses Projekt steht unter der [MIT Lizenz](LICENSE).
