use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::watch::Sender;
use slint::{ComponentHandle, Model};
use crate::{AppWindow, LogEntry};
use crate::storage::{AppStorage, PrivacyStatus, save_storage};

pub fn setup_ui(
    storage: &Arc<Mutex<AppStorage>>,
    current_delete_original: bool,
    current_privacy_status: PrivacyStatus,
    path_tx: &Arc<Sender<Option<PathBuf>>>,
    del_tx: Arc<Sender<bool>>,
    ps_tx: Arc<Sender<PrivacyStatus>>,
    mut log_rx: tokio::sync::mpsc::Receiver<LogEntry>,
) -> (AppWindow, slint::Weak<AppWindow>, tray_icon::TrayIcon) {
    let (tray_icon, open_item_id, quit_item_id) = setup_tray_icon();

    let ui = AppWindow::new().expect("Fehler beim erstellen vom UI");
    let ui_weak = ui.as_weak();

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

    let storage_clone_delete_original = Arc::clone(storage);
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
