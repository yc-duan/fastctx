use fastctx_install::{apply, service, uninstall::Installation};
use fastctx_types::uninstall::{Hub, Item, Other, Plan};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Request {
    pub installation: Installation,
    pub plan: Plan,
    pub steps: Vec<Step>,
    #[serde(default)]
    pub system_results: Vec<fastctx_types::uninstall::Event>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Step {
    pub item: Item,
    pub action: Action,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Action {
    Stop,
    Host(apply::Integration),
    ClaudeCache(PathBuf),
    ClaudeWorkCache {
        directory: PathBuf,
        diagnostics: PathBuf,
        working_directories: Vec<String>,
        sessions: Vec<String>,
        temporary: bool,
    },
    ClaudeSessionCache {
        path: PathBuf,
        job: Option<PathBuf>,
        sessions: Vec<String>,
    },
    Service(service::Registration),
    ServicePart(String),
    /// The inbound rules, by name, a node kept for direct connections.
    DirectRules(Vec<String>),
    Legacy(fastctx_install::uninstall::Legacy),
    Trust {
        home: PathBuf,
        key: String,
    },
    Logout {
        identity: PathBuf,
        directory: PathBuf,
        account: Option<fastctx_types::AccountName>,
    },
    Programs,
    WorldData,
    HubData(PathBuf),
    Path,
    Delete(PathBuf),
    Application,
    ApplicationData(PathBuf),
    Package,
    /// The account, by SID, that 1.0.1-beta.3 made for a managed computer's
    /// World (`fastctx_install::service::managed`).
    Account(String),
}

/// The hubs whose directories a World keeps under `net`, by id.
#[cfg(windows)]
fn hub_ids(net: &std::path::Path) -> anyhow::Result<Vec<String>> {
    Ok(crate::entries(net)?
        .into_iter()
        .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
        .filter(|name| fastctx_net::ids::HubId::try_from(name.clone()).is_ok())
        .collect())
}

pub fn preview(mut installation: Installation, everything: bool) -> anyhow::Result<Request> {
    installation.validate()?;
    // Of the application's possible data directories, only those that are
    // there are planned for or named among what stays.
    let mut present = Vec::new();
    for directory in std::mem::take(&mut installation.application_dirs) {
        if directory.try_exists()? {
            present.push(directory);
        }
    }
    installation.application_dirs = present;
    if let Some(mut request) = crate::read::<Request>(&installation.marker())?
        // A removal that keeps the data, finished or cut short, gives way to
        // a complete one, planned afresh from what is on this computer now.
        && (request.plan.everything || !everything)
    {
        anyhow::ensure!(
            request.plan.everything == everything,
            "an interrupted uninstall has a different mode; finish that mode first"
        );
        anyhow::ensure!(
            request.installation.root == installation.root && request.installation.home == installation.home,
            "the uninstall checkpoint belongs to a different user or installation"
        );
        refresh_application_data(&mut request, &installation);
        request.installation.application_process = installation.application_process;
        request.installation.application_control = installation.application_control;
        request.installation.graphical = installation.graphical;
        retain_legacy_archives(&mut request);
        crate::claude_cache::include_steps(&mut request);
        crate::claude_work_cache::include_steps(&mut request)?;
        crate::claude_session_cache::include_steps(&mut request)?;
        let marker = fastctx_types::layout::WorldDir::under(&request.installation.root).node_service();
        if marker.try_exists()?
            && !request.steps.iter().any(|step| matches!(&step.action, Action::Delete(path) if path == &marker))
        {
            let item = Item {
                id: "node-service-marker".into(),
                kind: "service".into(),
                what: "Remove the node's boot-service marker".into(),
                location: Some(marker.display().to_string()),
                admin: false,
            };
            request.plan.steps.push(item.clone());
            request.steps.push(Step { item, action: Action::Delete(marker) });
        }
        return Ok(request);
    }
    let root = &installation.root;
    let world = fastctx_types::layout::WorldDir::under(root);
    let places = installation.places(std::env::current_exe()?);
    let homes = apply::integrations(&places)?;
    let registrations = service::registrations(&installation)?;
    let managed = crate::managed(&world)?;
    let mut plan = Plan { everything, steps: Vec::new(), keeps: Vec::new(), hub: None, other: None, managed };
    let mut steps = Vec::new();
    let mut add = |kind: &str, what: &str, location: String, admin: bool, action: Action| {
        let item = Item {
            id: format!("step-{}", steps.len() + 1),
            kind: kind.into(),
            what: what.into(),
            location: Some(location),
            admin,
        };
        plan.steps.push(item.clone());
        steps.push(Step { item, action });
    };
    if root.try_exists()? {
        add(
            "runtime",
            "Stop this World's node, agent sessions and supervised jobs",
            world.path().display().to_string(),
            false,
            Action::Stop,
        );
    }
    if everything {
        let mut identities = vec![(world.path().to_path_buf(), world.net())];
        identities.extend(
            installation
                .application_dirs
                .iter()
                .map(|directory| (directory.join("device"), directory.join("device/hubs"))),
        );
        for (identity, hubs) in identities {
            for hub in crate::entries(&hubs)? {
                // A hub's directory is named for the hub's id; the node keeps
                // its own files beside them, as the node's own reading skips.
                let named = hub
                    .file_name()
                    .to_str()
                    .is_some_and(|name| fastctx_net::ids::HubId::try_from(name.to_string()).is_ok());
                if !named {
                    continue;
                }
                let directory = hub.path();
                let Some(membership) =
                    crate::read::<fastctx_net::link::state::Membership>(&directory.join("membership.json"))?
                else {
                    continue;
                };
                let view = crate::read::<fastctx_net::directory::View>(&directory.join("view.json"))?;
                let accounts = view
                    .as_ref()
                    .and_then(|view| view.me())
                    .map(|me| me.logins.iter().cloned().collect::<Vec<_>>())
                    .unwrap_or_default();
                if accounts.is_empty() {
                    add(
                        "logout",
                        "Ask this hub to end every login and machine registration",
                        membership.hub_name.to_string(),
                        false,
                        Action::Logout { identity: identity.clone(), directory, account: None },
                    );
                } else {
                    for account in accounts {
                        add(
                            "logout",
                            "End this login and its machine registration; the hub keeps historical records",
                            format!("{} · {account}", membership.hub_name),
                            false,
                            Action::Logout {
                                identity: identity.clone(),
                                directory: directory.clone(),
                                account: Some(account),
                            },
                        );
                    }
                }
            }
        }
    }
    for registration in &registrations {
        add(
            if registration.kind == service::Kind::Node { "service" } else { "hub-service" },
            "Stop and uninstall the boot service and its protected files",
            registration.name.clone(),
            true,
            Action::Service(registration.clone()),
        );
        add(
            "task",
            "Remove the service's independent recovery check",
            format!("{}-check", registration.name),
            true,
            Action::ServicePart(registration.name.clone()),
        );
        if cfg!(windows) && registration.kind == service::Kind::Hub {
            add(
                "firewall",
                "Remove the hub's inbound firewall rule",
                format!("fastctx-{}", registration.name),
                true,
                Action::ServicePart(registration.name.clone()),
            );
        }
    }
    // The rules a node kept for direct connections go in either removal:
    // they let in a program that goes too, and a node put back makes its
    // own again when the hub says so.
    #[cfg(windows)]
    for hub in hub_ids(&world.net())? {
        let earlier = fastctx_platform::account_name(&installation.user_sid)
            .ok()
            .map(|account| fastctx_install::direct_firewall::earlier_rule(&account, &hub));
        let names: Vec<String> = [Some(fastctx_install::direct_firewall::rule(&installation.user_sid, &hub)), earlier]
            .into_iter()
            .flatten()
            // A rule that cannot be looked for is removed if it is there.
            .filter(|name| fastctx_install::direct_firewall::present(name).unwrap_or(true))
            .collect();
        if !names.is_empty() {
            add(
                "firewall",
                "Remove the inbound firewall rule for direct connections",
                names.join("; "),
                true,
                Action::DirectRules(names),
            );
        }
    }
    for home in &homes {
        add(
            if home.host == apply::Host::Codex { "codex" } else { "claude" },
            "Withdraw the World's integration and restore recorded original values",
            home.home.display().to_string(),
            false,
            Action::Host(home.clone()),
        );
    }
    if everything && world.path().try_exists()? {
        let key = fastctx_hosts::claude::trust::uninstall_key(world.path())?;
        let mut claude_homes = vec![installation.claude_home.clone()];
        claude_homes.extend(homes.iter().filter(|home| home.host == apply::Host::Claude).map(|home| home.home.clone()));
        for about in abouts(&world)? {
            if let Some(about) = crate::read::<fastctx_types::residents::About>(&about)?
                && about.host.agent.as_str() == "claude"
            {
                claude_homes.push(about.home.unwrap_or_else(|| installation.claude_home.clone()));
            }
        }
        claude_homes.sort();
        claude_homes.dedup();
        for home in claude_homes {
            add(
                "claude-trust",
                "Remove the trust records of this World's own directories, where its sessions ran",
                home.display().to_string(),
                false,
                Action::Trust { home, key: key.clone() },
            );
        }
    }
    for legacy in fastctx_install::uninstall::legacy(&installation)? {
        add(
            if cfg!(windows) { "task" } else { "service" },
            "Stop and remove the unpublished fastctx node registration",
            legacy.name.clone(),
            cfg!(windows),
            Action::Legacy(legacy),
        );
    }
    if world.node_service().try_exists()? {
        add(
            "service",
            "Remove the node's boot-service marker",
            world.node_service().display().to_string(),
            false,
            Action::Delete(world.node_service()),
        );
    }
    let path_location =
        if cfg!(target_os = "linux") { installation.home.join(".local/bin/fastctx") } else { root.join("bin") };
    add(
        "path",
        "Remove only the terminal entry that names this World's program",
        path_location.display().to_string(),
        false,
        Action::Path,
    );
    add(
        "program",
        "Remove this World's stable, pinned and staged program assets",
        root.join("bin").display().to_string(),
        false,
        Action::Programs,
    );
    let mut hub_dirs = registrations
        .iter()
        .filter(|registration| registration.kind == service::Kind::Hub)
        .map(|registration| registration.directory.clone())
        .collect::<Vec<_>>();
    hub_dirs.extend(service::registered_hubs(root)?);
    if root.join("hub/hub.json").try_exists()? {
        hub_dirs.push(root.join("hub"));
    }
    hub_dirs.sort();
    hub_dirs.dedup();
    let mut hubs = Vec::new();
    for directory in hub_dirs {
        let metadata = crate::read::<serde_json::Value>(&directory.join("hub.json"))?;
        let name = metadata.as_ref().and_then(|value| value.get("name")).and_then(serde_json::Value::as_str);
        let Some(name) = name.map(str::to_owned) else {
            // A hub retired when its network was dissolved or moved keeps
            // only its log and why (`retired.json`): it hosts nothing, and
            // what it left goes with everything.
            if directory.join("retired.json").try_exists()? {
                if everything {
                    add(
                        "hub-data",
                        "Delete what a retired hub left: its log",
                        directory.display().to_string(),
                        true,
                        Action::HubData(directory.clone()),
                    );
                }
                continue;
            }
            anyhow::bail!("the local hub in {} has no readable world name", directory.display());
        };
        if everything {
            add(
                "hub-data",
                &format!("Delete the hosted world {name}, its hub, accounts and records"),
                directory.display().to_string(),
                true,
                Action::HubData(directory.clone()),
            );
        }
        hubs.push(Hub { name, dir: directory.display().to_string() });
    }
    for entry in crate::entries(root)? {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("legacy-archive-") {
            plan.keeps.push(keep(
                "user-files",
                "Old-version files with unproven or user-modified contents",
                &entry.path().display().to_string(),
            ));
        } else if everything && name == "hub-third" && entry.path().join("hub.json").try_exists()? {
            add(
                "hub-data",
                "Delete the third-version hub files its migration kept",
                entry.path().display().to_string(),
                false,
                Action::Delete(entry.path()),
            );
        } else if everything && name == "world" {
            add(
                "data",
                "Delete this World's identity, logins, projects and records",
                entry.path().display().to_string(),
                false,
                Action::WorldData,
            );
        } else if everything
            && matches!(
                name.as_ref(),
                "renewal"
                    | "licenses"
                    | "applied.json"
                    | "applied-files-pending.json"
                    | "applied-operation.json"
                    | "apply.lock"
                    | "uninstalled-hosts.json"
                    | "upgrade-clean.json"
                    | "config.toml"
                    | "service-node.log"
                    | "service-node.log.1"
                    | "server-install.lock"
                    | "hub-services.json"
                    | "upgrade-clean.lock"
                    | "cache"
                    | "updates"
            )
        {
            add(
                "data",
                "Delete this World's data or installation record",
                entry.path().display().to_string(),
                false,
                Action::Delete(entry.path()),
            );
        } else if everything && name.starts_with("applied-claude-plugin") && name.ends_with(".json") {
            add(
                "data",
                "Delete the World's Claude plugin receipt",
                entry.path().display().to_string(),
                false,
                Action::Delete(entry.path()),
            );
        }
    }
    if installation.application.is_some() {
        let deb = cfg!(target_os = "linux")
            && installation.application.as_deref() == Some(std::path::Path::new("/usr/bin/fastctx-app"));
        add(
            "app",
            "After the kernel finishes, exit Fastctx and run its package uninstaller",
            installation
                .application_image
                .as_ref()
                .or(installation.application.as_ref())
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "World application".into()),
            deb,
            if deb { Action::Package } else { Action::Application },
        );
        if everything {
            for directory in &installation.application_dirs {
                add(
                    "app",
                    "The package uninstaller will remove this application's device identity, preferences and WebView data after World exits",
                    directory.display().to_string(),
                    false,
                    Action::Application,
                );
            }
        }
    } else if everything {
        for directory in &installation.application_dirs {
            add(
                "app",
                "Delete the former application's device identity, preferences and WebView data",
                directory.display().to_string(),
                false,
                Action::ApplicationData(directory.clone()),
            );
        }
    }
    anyhow::ensure!(
        !(everything && installation.graphical && hubs.len() > 1),
        "this computer hosts multiple local worlds ({}); the graphical confirmation currently represents only one. Each hub must be shown before removal",
        hubs.iter().map(|hub| hub.name.as_str()).collect::<Vec<_>>().join(", ")
    );
    plan.hub = hubs.first().cloned();
    for hub in &hubs {
        if !everything {
            plan.keeps.push(keep("hub-data", "The hosted world's hub data", &hub.dir));
        }
    }
    if !everything {
        plan.keeps.push(keep(
            "data",
            "World identity, logins, projects, records and withdrawn-host recovery",
            &root.display().to_string(),
        ));
        for directory in &installation.application_dirs {
            plan.keeps.push(keep(
                "app",
                "World device identity and interface preferences",
                &directory.display().to_string(),
            ));
        }
    }
    // Named once, by the archive that holds them, as the removal reports them;
    // one kept beside a file on another volume, by its own place.
    let history = root.join("apply-history");
    let mut archived = false;
    for archive in fastctx_install::transaction::retained_archives(root)? {
        let location = if archive.starts_with(&history) {
            if std::mem::replace(&mut archived, true) {
                continue;
            }
            history.clone()
        } else {
            archive
        };
        plan.keeps.push(keep(
            "user-files",
            "User file versions retained by host publication transactions",
            &location.display().to_string(),
        ));
    }
    // The account 1.0.1-beta.3 made for a managed computer's World goes with
    // that World, last, once nothing of it runs.
    #[cfg(windows)]
    if everything && service::managed::made()?.as_deref() == Some(installation.user_sid.as_str()) {
        let item = Item {
            id: "managed-account".into(),
            kind: "account".into(),
            what: "Remove the account this World lived in".into(),
            location: Some(service::managed::ACCOUNT.into()),
            admin: true,
        };
        plan.steps.push(item.clone());
        steps.push(Step { item, action: Action::Account(installation.user_sid.clone()) });
    }
    let programs = other_programs(&places, installation.application.as_deref())?;
    for program in &programs {
        plan.keeps.push(keep(
            "other-program",
            &program.version.clone().unwrap_or_else(|| "Version unavailable".into()),
            &program.path,
        ));
    }
    plan.other = programs.first().cloned();
    let mut request = Request { installation, plan, steps, system_results: Vec::new() };
    crate::claude_cache::include_steps(&mut request);
    crate::claude_work_cache::include_steps(&mut request)?;
    crate::claude_session_cache::include_steps(&mut request)?;
    Ok(request)
}

fn keep(kind: &str, what: &str, location: &str) -> Item {
    Item {
        id: format!("keep-{kind}-{location}"),
        kind: kind.into(),
        what: what.into(),
        location: Some(location.into()),
        admin: false,
    }
}

fn refresh_application_data(request: &mut Request, installation: &Installation) {
    request.installation.application_dirs.extend(installation.application_dirs.iter().cloned());
    request.installation.application_dirs.sort();
    request.installation.application_dirs.dedup();
    if !request.plan.everything {
        for directory in &request.installation.application_dirs {
            let location = directory.display().to_string();
            if !request.plan.keeps.iter().any(|item| item.location.as_deref() == Some(&location)) {
                request.plan.keeps.push(keep("app", "World device identity and interface preferences", &location));
            }
        }
        return;
    }
    let direct = installation.application.is_none() && !installation.graphical;
    for directory in &request.installation.application_dirs {
        let location = directory.display().to_string();
        if let Some(step) = request.steps.iter_mut().find(|step| {
            matches!(step.action, Action::Application | Action::ApplicationData(_))
                && step.item.location.as_deref() == Some(&location)
        }) {
            if direct && matches!(step.action, Action::Application) {
                let old = step.item.id.clone();
                step.item.id = format!("application-data-{old}");
                step.item.what = "Delete the former application's device identity, preferences and WebView data".into();
                step.action = Action::ApplicationData(directory.clone());
                if let Some(item) = request.plan.steps.iter_mut().find(|item| item.id == old) {
                    *item = step.item.clone();
                }
            }
            continue;
        }
        let mut index = request.steps.len() + 1;
        while request.steps.iter().any(|step| step.item.id == format!("application-data-{index}")) {
            index += 1;
        }
        let item = Item {
            id: format!("application-data-{index}"),
            kind: "app".into(),
            what: "Delete the former application's device identity, preferences and WebView data".into(),
            location: Some(location),
            admin: false,
        };
        request.plan.steps.push(item.clone());
        request.steps.push(Step {
            item,
            action: if direct { Action::ApplicationData(directory.clone()) } else { Action::Application },
        });
    }
}

pub(crate) fn is_legacy_archive(path: &std::path::Path, root: &std::path::Path) -> bool {
    path.parent() == Some(root)
        && path.file_name().is_some_and(|name| name.to_string_lossy().starts_with("legacy-archive-"))
}

fn retain_legacy_archives(request: &mut Request) {
    request.steps.retain(|step| {
        if let Action::Delete(path) = &step.action
            && is_legacy_archive(path, &request.installation.root)
        {
            request.plan.steps.retain(|item| item.id != step.item.id);
            let location = path.display().to_string();
            if !request.plan.keeps.iter().any(|item| item.location.as_deref() == Some(&location)) {
                request.plan.keeps.push(keep(
                    "user-files",
                    "Old-version files with unproven or user-modified contents",
                    &location,
                ));
            }
            false
        } else {
            true
        }
    });
}

fn other_programs(places: &apply::Places, application: Option<&std::path::Path>) -> anyhow::Result<Vec<Other>> {
    let mut programs = Vec::new();
    let Some(path) = std::env::var_os("PATH") else { return Ok(programs) };
    for directory in std::env::split_paths(&path) {
        for name in if cfg!(windows) { &["fastctx.exe", "fastctx.cmd"][..] } else { &["fastctx"][..] } {
            let program = directory.join(name);
            let carried = application
                .map(
                    |path| if path.is_dir() { path.to_path_buf() } else { path.parent().unwrap_or(path).to_path_buf() },
                )
                .map(|directory| directory.join(name));
            let installed = places.installed();
            let same_installed = program.is_file()
                && installed.is_file()
                && std::fs::canonicalize(&program)? == std::fs::canonicalize(&installed)?;
            if !program.is_file()
                || program.starts_with(&places.root)
                || same_installed
                || carried.as_ref() == Some(&program)
            {
                continue;
            }
            if programs.iter().any(|other: &Other| other.path == program.display().to_string()) {
                continue;
            }
            // Version queries have no World lifecycle. Removing the upgrade
            // handoff variable also keeps an older launcher from finalizing it.
            let output =
                std::process::Command::new(&program).arg("--version").env_remove("FASTCTX_UPDATE_FINALIZE").output()?;
            let version = output.status.success().then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned());
            programs.push(Other { path: program.display().to_string(), version });
        }
    }
    Ok(programs)
}

/// Where each resident on this machine says who it is: in its conversation's
/// home, or, in a World no node of this version has started yet, in the
/// directories of the earlier layout.
pub(crate) fn abouts(world: &fastctx_types::layout::WorldDir) -> std::io::Result<Vec<std::path::PathBuf>> {
    let mut abouts = Vec::new();
    for kept in world.kept()? {
        if matches!(kept.kind, fastctx_types::layout::Kind::Home(_)) {
            abouts.push(kept.directory.about());
        }
    }
    for parent in world.earlier().parents()? {
        for resident in crate::entries(&parent)? {
            for about in [resident.path().join("about"), resident.path().join("archive").join("about")] {
                if about.try_exists()? {
                    abouts.push(about);
                }
            }
        }
    }
    Ok(abouts)
}
