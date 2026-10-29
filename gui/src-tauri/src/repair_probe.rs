//! The repairer checks WebView startup without opening the user's data profile.

use std::path::PathBuf;

pub fn run(receipt: PathBuf, profile: PathBuf) -> Result<(), String> {
    use tauri::Manager as _;
    let mut context = tauri::generate_context!();
    context.config_mut().app.windows.clear();
    let failed_receipt = receipt.clone();
    let result = tauri::Builder::default()
        .setup(move |app| {
            if let Err(error) = resources(app.handle()) {
                let _ = fastctx_install::renewal::write_json(
                    &receipt,
                    &fastctx_install::repair::application::Probe {
                        version: env!("CARGO_PKG_VERSION").into(),
                        error: Some(error),
                    },
                );
                app.handle().exit(1);
                return Ok(());
            }
            let page_receipt = receipt.clone();
            let window =
                tauri::WebviewWindowBuilder::new(app, "repair-probe", tauri::WebviewUrl::App("index.html".into()))
                    .visible(false)
                    .data_directory(profile.clone())
                    .on_page_load(move |window, payload| {
                        if payload.event() == tauri::webview::PageLoadEvent::Finished {
                            let result = fastctx_install::renewal::write_json(
                                &page_receipt,
                                &fastctx_install::repair::application::Probe {
                                    version: env!("CARGO_PKG_VERSION").into(),
                                    error: None,
                                },
                            );
                            window.app_handle().exit(if result.is_ok() { 0 } else { 1 });
                        }
                    })
                    .build();
            if let Err(error) = window {
                let _ = fastctx_install::renewal::write_json(
                    &receipt,
                    &fastctx_install::repair::application::Probe {
                        version: env!("CARGO_PKG_VERSION").into(),
                        error: Some(format!("opening isolated WebView: {error}")),
                    },
                );
                app.handle().exit(1);
            }
            Ok(())
        })
        .build(context);
    match result {
        Ok(app) => {
            app.run(|_, _| {});
            Ok(())
        }
        Err(error) => {
            let why = format!("opening repair probe window: {error}");
            let _ = fastctx_install::renewal::write_json(
                &failed_receipt,
                &fastctx_install::repair::application::Probe {
                    version: env!("CARGO_PKG_VERSION").into(),
                    error: Some(why.clone()),
                },
            );
            Err(why)
        }
    }
}

fn resources(app: &tauri::AppHandle) -> Result<(), String> {
    if !fastctx_platform::RELEASED {
        return Ok(());
    }
    use tauri::Manager as _;
    let root = app.path().resource_dir().map_err(|e| format!("locating application resources: {e}"))?;
    for name in [
        "licenses/LICENSE-APACHE",
        "licenses/NOTICE",
        "licenses/THIRD_PARTY_LICENSES.md",
        "linux/fastctx-x86_64-unknown-linux-gnu.tar.gz",
        "linux/fastctx-aarch64-unknown-linux-gnu.tar.gz",
    ] {
        let path = root.join(name);
        let metadata =
            std::fs::metadata(&path).map_err(|e| format!("checking application resource {}: {e}", path.display()))?;
        if !metadata.is_file() || metadata.len() == 0 {
            return Err(format!("application resource {} is missing or empty", path.display()));
        }
    }
    Ok(())
}
