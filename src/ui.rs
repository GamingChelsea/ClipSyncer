use crate::storage::{AppStorage, PrivacyStatus, VideoEncoder, save_storage};
use crate::{AppWindow, LogEntry, VideoChannelEntry, VideoEntry};
use slint::{ComponentHandle, Model, SharedPixelBuffer};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::watch::Sender;

pub fn update_max_uploads_limit(ui: &AppWindow, storage: &AppStorage) -> i32 {
    let has_second_acc = match &storage.token_cache_2 {
        Some(s) if !s.trim().is_empty() && s.trim() != "[]" => true,
        _ => false,
    };

    let is_main_selected = storage.active_account == 0;

    let limit = if !has_second_acc || is_main_selected {
        6
    } else {
        12
    };

    ui.set_max_uploads_limit(limit);

    let mut current_val = ui.get_max_uploads_value();
    if current_val > limit {
        current_val = limit;
        ui.set_max_uploads_value(limit);
    }

    current_val
}

pub fn setup_ui(
    storage: &Arc<Mutex<AppStorage>>,
    current_delete_original: bool,
    current_notify: bool,
    current_privacy_status: PrivacyStatus,
    path_tx: &Arc<Sender<Option<PathBuf>>>,
    del_tx: Arc<Sender<bool>>,
    not_tx: Arc<Sender<bool>>,
    ps_tx: Arc<Sender<PrivacyStatus>>,
    active_account_tx: Arc<Sender<usize>>,
    mut log_rx: tokio::sync::mpsc::Receiver<LogEntry>,
    video_tx: &Arc<tokio::sync::mpsc::Sender<VideoChannelEntry>>,
    mut video_rx: tokio::sync::mpsc::Receiver<VideoChannelEntry>,
) -> (AppWindow, slint::Weak<AppWindow>, tray_icon::TrayIcon) {
    let (tray_icon, open_item_id, quit_item_id) = setup_tray_icon();

    let ui = AppWindow::new().expect("Fehler beim erstellen vom UI");
    let ui_weak = ui.as_weak();

    let language = {
        let guard = storage.lock().expect("Fehler beim Lesen von AppStorage");
        guard.language.clone()
    };
    let i18n_strings = crate::i18n::get_i18n_strings(&language);
    ui.set_i18n(i18n_strings.clone());
    ui.set_status_text(i18n_strings.status_waiting);

    let has_token = {
        let guard = storage.lock().expect("Fehler beim Lesen von AppStorage");
        guard.token_cache.is_some()
    };
    ui.set_logged_in(has_token);

    let storage_clone_secret = Arc::clone(storage);
    let ui_weak_secret = ui_weak.clone();
    ui.on_select_client_secret(move || {
        let storage_for_thread = Arc::clone(&storage_clone_secret);
        let ui_weak_for_thread = ui_weak_secret.clone();
        std::thread::spawn(move || {
            let result = rfd::FileDialog::new()
                .set_title("Wähle deine client_secret.json aus")
                .add_filter("JSON", &["json"])
                .pick_file();

            if let Some(file_path) = result {
                let rt = tokio::runtime::Runtime::new().expect("Tokio Runtime Fehler in Thread");
                let secret_result =
                    rt.block_on(async { yup_oauth2::read_application_secret(&file_path).await });

                if let Ok(secret) = secret_result {
                    if let Ok(mut guard) = storage_for_thread.lock() {
                        guard.client_secret = Some(secret);
                    }
                    save_storage(&storage_for_thread);
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = ui_weak_for_thread.upgrade() {
                            ui.set_needs_client_secret(false);
                        }
                    });
                }
            }
        });
    });

    let storage_clone = Arc::clone(storage);
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

    let storage_upload = Arc::clone(storage);
    let path_tx_upload = path_tx.clone();
    ui.on_upload_now(move || {
        let storage_for_thread = Arc::clone(&storage_upload);
        let path_tx_for_thread = path_tx_upload.clone();
        std::thread::spawn(move || {
            if let Ok(mut storage_ok) = storage_for_thread.lock() {
                storage_ok.upload_all = true;
                storage_ok.last_upload_date = chrono::Local::now() - chrono::Duration::hours(4);
                if let Some(path) = &storage_ok.clip_location {
                    let _ = &path_tx_for_thread.send_replace(Some(path.clone()));
                }
            }
        });
    });

    let ui_weak_log = ui_weak.clone();
    tokio::spawn(async move {
        while let Some(new_log) = log_rx.recv().await {
            let ui_handle = ui_weak_log.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = ui_handle.upgrade() {
                    let mut current_logs: Vec<LogEntry> = ui.get_logs().iter().collect();

                    if current_logs.len() >= 50 {
                        current_logs.remove(0);
                    }

                    current_logs.push(new_log);
                    let model = std::rc::Rc::new(slint::VecModel::from(current_logs));
                    ui.set_logs(model.into());
                }
            });
        }
    });

    let ui_weak_videos = ui_weak.clone();
    tokio::spawn(async move {
        while let Some(entry) = video_rx.recv().await {
            let ui_handle = ui_weak_videos.clone();

            let title = entry.title.clone();
            let link = entry.link.clone();
            let visibility = entry.visibility.clone();
            let thumbnail_url = entry.thumbnail_url.clone();

            let _ = slint::invoke_from_event_loop({
                let ui_handle = ui_handle.clone();
                let title = title.clone();
                let link = link.clone();
                let visibility = visibility.clone();
                let thumbnail_bytes = entry.thumbnail_bytes.clone();
                move || {
                    if let Some(ui) = ui_handle.upgrade() {
                        let mut current_videos: Vec<VideoEntry> = ui.get_videos().iter().collect();

                        let thumbnail_image = if let Some(bytes) = &thumbnail_bytes {
                            if let Ok(img) = image::load_from_memory(bytes) {
                                let rgba = img.to_rgba8();
                                let (w, h) = rgba.dimensions();
                                slint::Image::from_rgba8(SharedPixelBuffer::clone_from_slice(
                                    rgba.as_raw(),
                                    w,
                                    h,
                                ))
                            } else {
                                slint::Image::default()
                            }
                        } else {
                            slint::Image::default()
                        };

                        if let Some(item) = current_videos.iter_mut().find(|v| v.link == link) {
                            item.title = title.clone().into();
                            item.visibility = visibility.clone().into();
                            if thumbnail_bytes.is_some() {
                                item.thumbnail = thumbnail_image;
                            }
                        } else {
                            let video_entry = VideoEntry {
                                title: title.clone().into(),
                                link: link.clone().into(),
                                visibility: visibility.clone().into(),
                                thumbnail: thumbnail_image,
                            };

                            if current_videos.len() >= 50 {
                                current_videos.remove(0);
                            }
                            current_videos.push(video_entry);
                        }

                        let model = std::rc::Rc::new(slint::VecModel::from(current_videos));
                        ui.set_videos(model.into());
                    }
                }
            });

            if entry.thumbnail_bytes.is_none() && !thumbnail_url.is_empty() {
                tokio::spawn(async move {
                    let mut img_bytes: Option<Vec<u8>> = None;
                    let max_attempts = 5;
                    let delay = std::time::Duration::from_secs(10);

                    for attempt in 1..=max_attempts {
                        match reqwest::get(&thumbnail_url).await {
                            Ok(response) if response.status().is_success() => {
                                if let Ok(bytes) = response.bytes().await {
                                    img_bytes = Some(bytes.to_vec());
                                    break;
                                }
                            }
                            _ => {
                                tracing::info!(
                                    "Thumbnail für {} nicht verfügbar (Versuch {}/{}). Versuche erneut in ein paar Sekunden...",
                                    title,
                                    attempt,
                                    max_attempts
                                );
                            }
                        }
                        if attempt < max_attempts {
                            tokio::time::sleep(delay).await;
                        }
                    }

                    if let Some(bytes) = img_bytes {
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ui) = ui_handle.upgrade() {
                                let mut current_video: Vec<VideoEntry> =
                                    ui.get_videos().iter().collect();

                                if let Some(item) =
                                    current_video.iter_mut().find(|v| v.link == link)
                                {
                                    if let Ok(img) = image::load_from_memory(&bytes) {
                                        let rgba = img.to_rgba8();
                                        let (w, h) = rgba.dimensions();

                                        item.thumbnail = slint::Image::from_rgba8(
                                            SharedPixelBuffer::clone_from_slice(
                                                rgba.as_raw(),
                                                w,
                                                h,
                                            ),
                                        );

                                        let model =
                                            std::rc::Rc::new(slint::VecModel::from(current_video));
                                        ui.set_videos(model.into());
                                    }
                                }
                            }
                        });
                    }
                });
            }
        }
    });

    let storage_clone_notify_original = Arc::clone(storage);
    let not_tx_clone = not_tx.clone();
    ui.on_notify_changed(move |new_value| {
        let _ = not_tx_clone.send(new_value);
        let storage_for_thread = Arc::clone(&storage_clone_notify_original);
        std::thread::spawn(move || {
            if let Ok(mut guard) = storage_for_thread.lock() {
                guard.notify = new_value;
            }
            save_storage(&storage_for_thread);
        });
    });
    ui.set_delete_original(current_notify);

    let storage_clone_delete_original = Arc::clone(storage);
    let del_tx_clone = del_tx.clone();
    ui.on_delete_original_changed(move |new_value| {
        let _ = del_tx_clone.send(new_value);
        let storage_for_thread = Arc::clone(&storage_clone_delete_original);
        std::thread::spawn(move || {
            if let Ok(mut guard) = storage_for_thread.lock() {
                guard.notify = new_value;
            }
            save_storage(&storage_for_thread);
        });
    });
    ui.set_delete_original(current_delete_original);

    ui.on_link_clicked(move |link| {
        let _ = webbrowser::open(&link.as_str());
    });

    let storage_clone_privacy_status = Arc::clone(storage);
    let ps_tx_clone = ps_tx.clone();
    ui.on_privacy_change(move |index| {
        let _ = ps_tx_clone.send(PrivacyStatus::new(index));
        let storage_for_thread = Arc::clone(&storage_clone_privacy_status);
        std::thread::spawn(move || {
            if let Ok(mut guard) = storage_for_thread.lock() {
                guard.privacy_status = PrivacyStatus::new(index);
            }
            save_storage(&storage_for_thread);
        });
    });
    ui.set_visibility_selection_index(
        (current_privacy_status as usize)
            .try_into()
            .expect("Fehler beim Setzen vom PrivacyStatus"),
    );

    let current_video_encoder = {
        let guard = storage.lock().expect("Fehler beim Lesen von AppStorage");
        guard.video_encoder
    };

    let storage_clone_video_encoder = Arc::clone(storage);
    ui.on_encoder_change(move |index| {
        let storage_for_thread = Arc::clone(&storage_clone_video_encoder);
        std::thread::spawn(move || {
            if let Ok(mut guard) = storage_for_thread.lock() {
                guard.video_encoder = VideoEncoder::new(index);
            }
            save_storage(&storage_for_thread);
        });
    });
    ui.set_encoder_selection_index(
        (current_video_encoder as usize)
            .try_into()
            .expect("Fehler beim Setzen vom VideoEncoder"),
    );

    let current_active_account = {
        let guard = storage.lock().expect("Fehler beim Lesen von AppStorage");
        guard.active_account
    };

    let current_max_uploads = {
        let guard = storage.lock().expect("Fehler beim Lesen von AppStorage");
        guard.max_uploads_per_day
    };
    ui.set_max_uploads_value(current_max_uploads as i32);
    let clamped_val = {
        let guard = storage.lock().expect("Fehler");
        update_max_uploads_limit(&ui, &guard)
    };
    if clamped_val as usize != current_max_uploads {
        if let Ok(mut guard) = storage.lock() {
            guard.max_uploads_per_day = clamped_val as usize;
        }
        save_storage(storage);
    }

    let storage_clone_active_account = Arc::clone(storage);
    let active_account_tx_clone = active_account_tx.clone();
    let ui_weak_active = ui_weak.clone();
    ui.on_active_account_change(move |index| {
        let active_idx = index as usize;
        let _ = active_account_tx_clone.send(active_idx);
        let storage_for_thread = Arc::clone(&storage_clone_active_account);
        let ui_weak_for_thread = ui_weak_active.clone();
        std::thread::spawn(move || {
            let mut current_max = 6;
            let mut save_needed = false;
            if let Ok(mut guard) = storage_for_thread.lock() {
                guard.active_account = active_idx;

                let has_second_acc = match &guard.token_cache_2 {
                    Some(s) if !s.trim().is_empty() && s.trim() != "[]" => true,
                    _ => false,
                };
                let limit = if !has_second_acc || active_idx == 0 {
                    6
                } else {
                    12
                };
                if guard.max_uploads_per_day > limit {
                    guard.max_uploads_per_day = limit;
                }
                current_max = guard.max_uploads_per_day;
                save_needed = true;
            }
            if save_needed {
                save_storage(&storage_for_thread);
            }

            let storage_for_ui = Arc::clone(&storage_for_thread);
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = ui_weak_for_thread.upgrade() {
                    if let Ok(guard) = storage_for_ui.lock() {
                        update_max_uploads_limit(&ui, &guard);
                        ui.set_max_uploads_value(current_max as i32);
                    }
                }
            });
        });
    });
    ui.set_active_account_index(current_active_account as i32);

    let storage_clone_max_uploads = Arc::clone(storage);
    ui.on_max_uploads_changed(move |val| {
        let storage_for_thread = Arc::clone(&storage_clone_max_uploads);
        std::thread::spawn(move || {
            if let Ok(mut guard) = storage_for_thread.lock() {
                guard.max_uploads_per_day = val as usize;
            }
            save_storage(&storage_for_thread);
        });
    });

    let storage_clone_import = Arc::clone(storage);
    let ui_weak_import = ui_weak.clone();
    let path_tx_import = path_tx.clone();
    let del_tx_import = del_tx.clone();
    let not_tx_import = not_tx.clone();
    let ps_tx_import = ps_tx.clone();
    let active_account_tx_import = active_account_tx.clone();
    let video_tx_import = video_tx.clone();

    ui.on_import_config(move || {
        let storage_for_thread = Arc::clone(&storage_clone_import);
        let ui_weak_for_thread = ui_weak_import.clone();
        let path_tx_for_thread = path_tx_import.clone();
        let del_tx_for_thread = del_tx_import.clone();
        let not_tx_for_thread = not_tx_import.clone();
        let ps_tx_for_thread = ps_tx_import.clone();
        let active_account_tx_for_thread = active_account_tx_import.clone();
        let video_tx_for_thread = video_tx_import.clone();

        std::thread::spawn(move || {
            let result = rfd::FileDialog::new()
                .set_title("Wähle deine config.ron aus")
                .add_filter("RON Config", &["ron"])
                .pick_file();

            if let Some(file_path) = result {
                let ron_content = match std::fs::read_to_string(&file_path) {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::error!("Fehler beim Lesen der ausgewählten Datei: {:?}", e);
                        return;
                    }
                };

                let new_storage: AppStorage = match ron::from_str(&ron_content) {
                    Ok(parsed) => parsed,
                    Err(e) => {
                        tracing::error!("Fehler beim Parsen der importierten Config: {:?}", e);
                        return;
                    }
                };

                {
                    if let Ok(mut guard) = storage_for_thread.lock() {
                        *guard = new_storage;
                    }
                }

                save_storage(&storage_for_thread);

                let (clip_loc, del_orig, notify_val, privacy, active_acc, max_uploads, encoder_idx, has_token, needs_secret, uploaded_videos, language) = {
                    let guard = storage_for_thread.lock().unwrap();
                    (
                        guard.clip_location.clone(),
                        guard.delete_original,
                        guard.notify,
                        guard.privacy_status,
                        guard.active_account,
                        guard.max_uploads_per_day,
                        guard.video_encoder,
                        guard.token_cache.is_some(),
                        guard.client_secret.is_none(),
                        guard.uploaded_videos.clone(),
                        guard.language.clone(),
                    )
                };

                let _ = path_tx_for_thread.send_replace(clip_loc.clone());
                let _ = del_tx_for_thread.send(del_orig);
                let _ = not_tx_for_thread.send(notify_val);
                let _ = ps_tx_for_thread.send(privacy);
                let _ = active_account_tx_for_thread.send(active_acc);

                for video in uploaded_videos.iter() {
                    let video_entry = VideoChannelEntry {
                        title: video.title.clone(),
                        link: video.link.clone(),
                        visibility: video.visibility.clone(),
                        thumbnail_url: video.thumbnail_url.clone(),
                        thumbnail_bytes: video.thumbnail_bytes.clone(),
                    };
                    let _ = video_tx_for_thread.try_send(video_entry);
                }

                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = ui_weak_for_thread.upgrade() {
                        let path_str = clip_loc.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
                        ui.set_selected_path(path_str.into());
                        ui.set_delete_original(del_orig);
                        ui.set_notify(notify_val);
                        ui.set_visibility_selection_index(privacy as usize as i32);
                        ui.set_encoder_selection_index(encoder_idx as usize as i32);
                        ui.set_active_account_index(active_acc as i32);
                        ui.set_max_uploads_value(max_uploads as i32);
                        ui.set_logged_in(has_token);
                        ui.set_needs_client_secret(needs_secret);

                        let i18n_strings = crate::i18n::get_i18n_strings(&language);
                        ui.set_i18n(i18n_strings.clone());
                        ui.set_status_text(i18n_strings.status_waiting);
                    }
                });
            }
        });
    });

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
                    show_slint_window(ui_tray_handle_1.clone());
                }
            }
        }
    });

    if let Some(path) = &storage
        .lock()
        .expect("Fehler beim Lesen von AppStorage")
        .clip_location
    {
        ui.set_selected_path(path.to_string_lossy().into_owned().into());
    };

    let ui_tray_handle_2 = ui_weak.clone();
    let storage_clone_2 = Arc::clone(storage);
    std::thread::spawn(move || {
        let menu_receiver = tray_icon::menu::MenuEvent::receiver();
        let ui_handle = ui_tray_handle_2.clone();
        let storage_2_for_thread = Arc::clone(&storage_clone_2);

        while let Ok(event) = menu_receiver.recv() {
            if event.id == quit_item_id {
                save_storage(&storage_2_for_thread);
                let _ = slint::invoke_from_event_loop(|| {
                    slint::quit_event_loop().expect("Fehler den Event Loop zu schließen")
                });
            } else if event.id == open_item_id {
                show_slint_window(ui_handle.clone());
            }
        }
    });

    for video in storage
        .clone()
        .lock()
        .expect("Fehler beim Lesen von AppStorage")
        .uploaded_videos
        .iter()
    {
        let video_entry = VideoChannelEntry {
            title: video.title.clone(),
            link: video.link.clone(),
            visibility: video.visibility.clone(),
            thumbnail_url: video.thumbnail_url.clone(),
            thumbnail_bytes: video.thumbnail_bytes.clone(),
        };
        let _ = video_tx.try_send(video_entry);
    }

    (ui, ui_weak, tray_icon)
}

pub fn show_slint_window(ui_handle: slint::Weak<AppWindow>) {
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(ui) = ui_handle.upgrade() {
            ui.show().expect("Fehler das Fenster anzuzeigen");
        }
    });
}

fn setup_tray_icon() -> (
    tray_icon::TrayIcon,
    tray_icon::menu::MenuId,
    tray_icon::menu::MenuId,
) {
    let image_data = include_bytes!("../assets/icon.png");
    let dynamic_image = image::load_from_memory(image_data).expect("Fehler beim öffnen vom Icon");
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

    let tray_icon = tray_icon::TrayIconBuilder::new()
        .with_tooltip("Clip Syncer")
        .with_icon(icon)
        .with_menu(Box::new(tray_menu.clone()))
        .with_menu_on_left_click(false)
        .build()
        .expect("Fehler beim erstellen des Tray Icons");
    (tray_icon, open_item_id, quit_item_id)
}
