//! Whether the main window's interface still answers. The page says it is
//! alive every couple of seconds (`ui_alive`, from `boot.ts`). A page that
//! says nothing for a while, its window shown and in front all that time,
//! is taken for one its engine is held in, a script that never yields (the
//! machine map's wait for its faces on WebKit drew nothing on macOS until
//! the app's own storage was cleared). The window is then made again, once
//! in the process's life, with the interface in its safe start: it opens on
//! the agents without bringing back where it was, and what was typed and
//! not sent stays where it is kept (`app/safe-start.ts`).

use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::Manager as _;

/// How long a page that has spoken may say nothing, its window in front.
const SILENT: Duration = Duration::from_secs(20);
/// How long a new page may take to say anything, its window in front: it
/// first waits for this machine's World to open (`startup/gate.ts`).
const FIRST: Duration = Duration::from_secs(45);
/// How often the shell looks.
const LOOK: Duration = Duration::from_secs(2);
/// Looks further apart than this: the machine slept, or the shell was held
/// up, and the page is given its time again.
const GAP: Duration = Duration::from_secs(10);
/// The page the window is made again with.
const SAFE: &str = "index.html?safe=1";

#[derive(Default)]
pub(crate) struct Watch(Mutex<Seen>);

#[derive(Default)]
struct Seen {
    /// When the page last spoke, or was last given its time: a new page,
    /// the window brought to the front, the machine waking.
    since: Option<Instant>,
    /// Whether this page has spoken at all.
    spoke: bool,
    /// Whether the window was made again already in this process's life.
    remade: bool,
}

/// The page in the main window still answers.
#[tauri::command]
pub(crate) fn ui_alive(webview: tauri::Webview, watch: tauri::State<'_, Watch>) {
    if webview.label() != "main" {
        return;
    }
    let mut seen = watch.0.lock().expect("never poisoned");
    seen.since = Some(Instant::now());
    seen.spoke = true;
}

/// A new page began loading in the main window: it is given its time to
/// speak from now.
pub(crate) fn page_started(app: &tauri::AppHandle) {
    let Some(watch) = app.try_state::<Watch>() else { return };
    let mut seen = watch.0.lock().expect("never poisoned");
    seen.since = Some(Instant::now());
    seen.spoke = false;
}

/// Looks at the main window's page every couple of seconds while the app
/// runs.
pub(crate) fn start(app: &tauri::AppHandle) {
    let watching = app.clone();
    let looking = std::thread::Builder::new().name("ui-watch".into()).spawn(move || {
        let app = watching;
        let mut last = Instant::now();
        loop {
            std::thread::sleep(LOOK);
            let now = Instant::now();
            let slept = now.duration_since(last) > GAP;
            last = now;
            let Some(window) = app.get_webview_window("main") else { continue };
            let front = window.is_visible().unwrap_or(false)
                && !window.is_minimized().unwrap_or(true)
                && window.is_focused().unwrap_or(false);
            let watch = app.state::<Watch>();
            let mut seen = watch.0.lock().expect("never poisoned");
            // A page whose window is not in front may be held back by the
            // engine for that alone: it is judged only while in front, from
            // when it came there.
            if slept || !front || seen.remade {
                seen.since = Some(now);
                continue;
            }
            let since = *seen.since.get_or_insert(now);
            let limit = if seen.spoke { SILENT } else { FIRST };
            let quiet = now.duration_since(since);
            if quiet < limit {
                continue;
            }
            seen.remade = true;
            drop(seen);
            remake(&app, quiet);
        }
    });
    if let Err(error) = looking {
        crate::startup::warning(app, "ui_watch", error.to_string());
    }
}

/// Makes the main window again with the interface's safe start, the old
/// one destroyed first: a page held in a script answers nothing, its
/// window and engine go with it.
fn remake(app: &tauri::AppHandle, quiet: Duration) {
    crate::startup::warning(
        app,
        "frontend",
        format!(
            "the interface said nothing for {} s with its window in front; its window is made again in its safe start",
            quiet.as_secs()
        ),
    );
    let Some(mut config) = app.config().app.windows.iter().find(|window| window.label == "main").cloned() else {
        return;
    };
    config.url = tauri::WebviewUrl::App(SAFE.into());
    let destroying = app.clone();
    if let Err(error) = app.run_on_main_thread(move || {
        if let Some(window) = destroying.get_webview_window("main")
            && let Err(error) = window.destroy()
        {
            crate::startup::warning(
                &destroying,
                "frontend",
                format!("closing the window that stopped answering: {error}"),
            );
        }
    }) {
        crate::startup::warning(app, "frontend", error.to_string());
        return;
    }
    // The label is free once the old window is gone.
    let making = app.clone();
    let waited = std::thread::Builder::new().name("ui-remake".into()).spawn(move || {
        for _ in 0..50 {
            if making.get_webview_window("main").is_none() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let built = making.clone();
        let run = making.run_on_main_thread(move || {
            let made = tauri::WebviewWindowBuilder::from_config(&built, &config).and_then(|window| window.build());
            match made {
                Ok(window) => {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
                Err(error) => crate::startup::warning(&built, "frontend", format!("making the window again: {error}")),
            }
        });
        if let Err(error) = run {
            crate::startup::warning(&making, "frontend", error.to_string());
        }
    });
    if let Err(error) = waited {
        crate::startup::warning(app, "frontend", error.to_string());
    }
}
