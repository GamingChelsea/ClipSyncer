#[cfg(windows)]
pub mod imp {
    use std::io::ErrorKind;
    use slint::ComponentHandle;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::windows::named_pipe::{ClientOptions, ServerOptions};
    use tracing::{error, info};

    const PIPE_NAME: &str = r"\\.\pipe\ClipSyncer-SingleInstance";

    pub enum InstanceCheck {
        Primary,
        Secondary,
    }

    pub fn check_or_notify() -> InstanceCheck {
        let rt = match tokio::runtime::Runtime::new() {
            Ok(r) => r,
            Err(_) => return InstanceCheck::Primary,
        };

        let is_secondary = rt.block_on(async {
            match ClientOptions::new().open(PIPE_NAME) {
                Ok(mut client) => {
                    info!("Bereits laufende ClipSyncer-Instanz gefunden. Sende Aktivierungssignal...");
                    let _ = client.write_all(b"show\n").await;
                    true
                }
                Err(e) if e.kind() == ErrorKind::NotFound => false,
                Err(_) => false,
            }
        });

        if is_secondary {
            InstanceCheck::Secondary
        } else {
            InstanceCheck::Primary
        }
    }

    pub fn start_ipc_server(ui_weak: slint::Weak<crate::AppWindow>) {
        tokio::spawn(async move {
            let mut server = match ServerOptions::new()
                .first_pipe_instance(true)
                .create(PIPE_NAME)
            {
                Ok(s) => s,
                Err(e) => {
                    error!("Konnte Named Pipe Server nicht starten: {:?}", e);
                    return;
                }
            };

            loop {
                if server.connect().await.is_ok() {
                    let mut buf = [0u8; 64];
                    let _ = server.read(&mut buf).await;

                    let ui_weak_clone = ui_weak.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = ui_weak_clone.upgrade() {
                            ui.show().unwrap_or_default();
                        }
                    });

                    server = match ServerOptions::new().create(PIPE_NAME) {
                        Ok(s) => s,
                        Err(_) => break,
                    };
                }
            }
        });
    }
}

#[cfg(not(windows))]
pub mod imp {
    use std::path::PathBuf;
    use slint::ComponentHandle;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{UnixListener, UnixStream};
    use tracing::{error, info};

    fn socket_path() -> PathBuf {
        std::env::temp_dir().join("clipsyncer_ipc.sock")
    }

    pub enum InstanceCheck {
        Primary,
        Secondary,
    }

    pub fn check_or_notify() -> InstanceCheck {
        let rt = match tokio::runtime::Runtime::new() {
            Ok(r) => r,
            Err(_) => return InstanceCheck::Primary,
        };

        let path = socket_path();
        let is_secondary = rt.block_on(async {
            if let Ok(mut stream) = UnixStream::connect(&path).await {
                let _ = stream.write_all(b"show\n").await;
                true
            } else {
                false
            }
        });

        if is_secondary {
            InstanceCheck::Secondary
        } else {
            InstanceCheck::Primary
        }
    }

    pub fn start_ipc_server(ui_weak: slint::Weak<crate::AppWindow>) {
        tokio::spawn(async move {
            let path = socket_path();
            let _ = std::fs::remove_file(&path);
            let listener = match UnixListener::bind(&path) {
                Ok(l) => l,
                Err(e) => {
                    error!("Konnte Unix Domain Socket nicht binden: {:?}", e);
                    return;
                }
            };

            loop {
                if let Ok((mut stream, _)) = listener.accept().await {
                    let mut buf = [0u8; 64];
                    let _ = stream.read(&mut buf).await;
                    let ui_weak_clone = ui_weak.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = ui_weak_clone.upgrade() {
                            ui.show().unwrap_or_default();
                        }
                    });
                }
            }
        });
    }
}

pub use imp::*;
