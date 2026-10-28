//! Native toasts with protocol activation, including Action Center and
//! cold starts. Only opaque record IDs leave the process in launch URLs.
//! Everything that asks Windows is done on the notification thread
//! (`apartment`), never on the caller's.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::Manager;
use windows::Data::Xml::Dom::XmlDocument;
use windows::Foundation::TypedEventHandler;
use windows::UI::Notifications::{
    NotificationSetting, ToastFailedEventArgs, ToastNotification, ToastNotificationManager,
};
use windows::core::HSTRING;

use super::apartment::Apartment;
use super::{Activation, Event, Presence, identity};

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Action {
    pub id: String,
    pub title: String,
    /// Handed to the page without bringing the window forward.
    #[serde(default)]
    pub quiet: bool,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Notice {
    pub title: String,
    pub body: String,
    pub route: String,
    #[serde(default)]
    pub actions: Vec<Action>,
}

pub struct Notifications {
    directory: PathBuf,
    records: Mutex<()>,
    apartment: Apartment,
}

fn xml(value: &str) -> String {
    value.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}

fn uri(id: &str, action: usize) -> String {
    format!("{}://notification/{id}/{action}", identity::SCHEME)
}

pub fn route(value: &str) -> Result<(), String> {
    if !value.starts_with("#/") || value.chars().any(char::is_control) {
        return Err("a notification destination must be an application hash beginning with #/".into());
    }
    Ok(())
}

impl Notifications {
    pub fn new(directory: PathBuf) -> Result<Self, String> {
        Ok(Self { directory, records: Mutex::new(()), apartment: Apartment::start()? })
    }

    pub async fn show(self: &Arc<Self>, app: &tauri::AppHandle, notice: Notice) -> Result<String, String> {
        let (this, app) = (Arc::clone(self), app.clone());
        self.apartment.run(move || this.submit(&app, notice)).await
    }

    pub async fn activate(self: &Arc<Self>, value: String) -> Result<Activation, String> {
        let this = Arc::clone(self);
        self.apartment.run(move || this.take(&value)).await
    }

    pub async fn remove(self: &Arc<Self>, id: String) -> Result<(), String> {
        let this = Arc::clone(self);
        self.apartment.run(move || this.forget(&id)).await
    }

    fn submit(&self, app: &tauri::AppHandle, notice: Notice) -> Result<String, String> {
        route(&notice.route)?;
        if notice.actions.len() > 5 {
            return Err("Windows supports at most five toast buttons".into());
        }
        let mut names = std::collections::HashSet::new();
        for action in &notice.actions {
            if action.id.is_empty() || !names.insert(&action.id) {
                return Err("notification button IDs must be nonempty and unique".into());
            }
        }
        identity::verify()?;
        let notifier = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(identity::APP_ID))
            .map_err(|e| format!("creating Windows notifier: {e}"))?;
        let id = uuid::Uuid::new_v4().simple().to_string();
        let buttons = notice
            .actions
            .iter()
            .enumerate()
            .map(|(index, action)| {
                format!(
                    "<action content=\"{}\" arguments=\"{}\" activationType=\"protocol\"/>",
                    xml(&action.title),
                    uri(&id, index + 1)
                )
            })
            .collect::<String>();
        let content = format!(
            "<toast activationType=\"protocol\" launch=\"{}\"><visual><binding template=\"ToastGeneric\"><text>{}</text><text>{}</text></binding></visual><actions>{buttons}</actions></toast>",
            uri(&id, 0),
            xml(&notice.title),
            xml(&notice.body),
        );
        // The Windows toast schema limits the complete XML payload to 5 KB.
        if content.len() > 5 * 1024 {
            return Err("notification XML exceeds the Windows 5 KB limit".into());
        }
        let document = XmlDocument::new().map_err(|e| e.to_string())?;
        document.LoadXml(&HSTRING::from(content)).map_err(|e| format!("building notification XML: {e}"))?;
        let toast = ToastNotification::CreateToastNotification(&document).map_err(|e| e.to_string())?;
        // Both identifiers are at most 16 characters on older Windows 10.
        toast.SetGroup(&HSTRING::from(&id[..16])).map_err(|e| e.to_string())?;
        toast.SetTag(&HSTRING::from(&id[16..])).map_err(|e| e.to_string())?;
        let app_failed = app.clone();
        let failed_id = id.clone();
        toast
            .Failed(&TypedEventHandler::<ToastNotification, ToastFailedEventArgs>::new(move |_, args| {
                let error = match args.as_ref() {
                    Some(args) => match args.ErrorCode() {
                        Ok(error) => format!("Windows could not display the notification: {error:?}"),
                        Err(error) => format!("reading the Windows notification failure: {error}"),
                    },
                    None => "Windows reported a notification failure without details".into(),
                };
                app_failed
                    .state::<Presence>()
                    .push(&app_failed, Event::NotificationFailed { notification_id: failed_id.clone(), error });
                Ok(())
            }))
            .map_err(|e| e.to_string())?;
        let _records = self.records.lock().expect("notification records");
        std::fs::create_dir_all(&self.directory).map_err(|e| e.to_string())?;
        let path = self.directory.join(format!("{id}.json"));
        let mut file =
            std::fs::OpenOptions::new().write(true).create_new(true).open(&path).map_err(|e| e.to_string())?;
        use std::io::Write;
        file.write_all(&serde_json::to_vec(&notice).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        // Unpackaged apps have no Setting entry until the first Show. Query
        // after submission so a new installation does not reject every toast
        // with E_ELEMENT_NOT_FOUND (CommunityToolkit ToastNotifierCompat).
        let submitted = notifier
            .Show(&toast)
            .map_err(|e| format!("showing notification: {e}"))
            .and_then(|()| notifier.Setting().map_err(|e| format!("reading Windows notification setting: {e}")))
            .and_then(|setting| {
                if setting == NotificationSetting::Enabled {
                    return Ok(());
                }
                let why = match setting {
                    NotificationSetting::DisabledForApplication => "disabled for Fastctx",
                    NotificationSetting::DisabledForUser => "disabled for this user",
                    NotificationSetting::DisabledByGroupPolicy => "disabled by group policy",
                    NotificationSetting::DisabledByManifest => "disabled by the application manifest",
                    _ => return Err(format!("unknown Windows notification setting: {setting:?}")),
                };
                Err(format!("Windows notifications are {why}"))
            });
        if let Err(error) = submitted {
            self.remove_history(&id).map_err(|cleanup| format!("{error}; removing notification: {cleanup}"))?;
            std::fs::remove_file(path)
                .map_err(|cleanup| format!("showing notification: {error}; removing its record: {cleanup}"))?;
            return Err(error);
        }
        // Windows retains the handlers after Show, as in Microsoft's
        // DesktopToasts sample. Holding toasts until Dismissed would accumulate
        // silent notifications, which have no promised on-screen dismissal.
        Ok(id)
    }

    fn take(&self, value: &str) -> Result<Activation, String> {
        let prefix = format!("{}://notification/", identity::SCHEME);
        let value = value.strip_prefix(&prefix).ok_or("unrecognized notification protocol")?;
        let (id, selected) = value.split_once('/').ok_or("missing notification action")?;
        validate_id(id)?;
        let selected: usize = selected.parse().map_err(|_| "invalid notification action")?;
        let _records = self.records.lock().expect("notification records");
        let path = self.directory.join(format!("{id}.json"));
        let notice: Notice =
            serde_json::from_slice(&std::fs::read(&path).map_err(|e| format!("reading notification {id}: {e}"))?)
                .map_err(|e| e.to_string())?;
        route(&notice.route)?;
        let action = if selected == 0 {
            None
        } else {
            Some(notice.actions.get(selected - 1).ok_or("unknown notification button")?)
        };
        self.remove_history(id)?;
        std::fs::remove_file(path).map_err(|e| e.to_string())?;
        Ok(Activation {
            route: Some(notice.route),
            quiet: action.is_some_and(|action| action.quiet),
            action: action.map(|action| action.id.clone()),
            notification_id: Some(id.to_owned()),
        })
    }

    fn forget(&self, id: &str) -> Result<(), String> {
        validate_id(id)?;
        let _records = self.records.lock().expect("notification records");
        self.remove_history(id)?;
        match std::fs::remove_file(self.directory.join(format!("{id}.json"))) {
            Ok(()) => (),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.to_string()),
        }
        Ok(())
    }

    fn remove_history(&self, id: &str) -> Result<(), String> {
        ToastNotificationManager::History()
            .map_err(|e| e.to_string())?
            .RemoveGroupedTagWithId(
                &HSTRING::from(&id[16..]),
                &HSTRING::from(&id[..16]),
                &HSTRING::from(identity::APP_ID),
            )
            .map_err(|e| e.to_string())
    }
}

fn validate_id(id: &str) -> Result<(), String> {
    if id.len() != 32 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("invalid notification ID".into());
    }
    Ok(())
}
