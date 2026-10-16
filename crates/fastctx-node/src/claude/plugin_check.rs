//! Claude Code checks the World's plugin as it checks one before loading it,
//! and the node says loudly when it would not load: its mod is read and
//! judged by Claude Code before it runs, and a mod it refuses tells the
//! World nothing at all. The check runs with a login the plugin is put into,
//! once for each version of the plugin: running Claude Code reads that
//! login, and syncs its remote settings, so it is not run at every start.
//! Where no login has the plugin, nothing loads it, and nothing is run.

use std::path::{Path, PathBuf};

use sha2::{Digest as _, Sha256};

use super::{Command, background, command};
use crate::Node;

/// The plugin's fingerprint Claude Code last accepted, beside the plugin.
const CHECKED: &str = "checked.json";

pub(super) fn run(node: &Node, plugin: &Path) {
    let context = match crate::logins::Context::of(node) {
        Ok(context) => context,
        Err(failure) => {
            node.log.line(format_args!("not asking Claude Code to check the World's plugin: {}", failure.message));
            return;
        }
    };
    let homes: Vec<PathBuf> = match fastctx_install::apply::integrations(&context.places) {
        Ok(found) => found
            .into_iter()
            .filter(|integration| integration.host == fastctx_install::apply::Host::Claude)
            .map(|integration| fastctx_hosts::inventory::plain(&integration.home))
            .collect(),
        Err(error) => {
            node.log.line(format_args!(
                "could not tell which Claude Code logins have the World's plugin, so none is asked to check it: {error}"
            ));
            return;
        }
    };
    let Some(home) = homes.first() else {
        node.log.line("no Claude Code login here has the World's plugin, so none is asked to check it");
        return;
    };
    let fingerprint = match fingerprint(plugin) {
        Ok(fingerprint) => fingerprint,
        Err(error) => {
            node.log.line(format_args!("reading the World's plugin to check it: {error}"));
            return;
        }
    };
    let record = plugin.with_file_name(CHECKED);
    if std::fs::read_to_string(&record).is_ok_and(|kept| kept.trim() == fingerprint) {
        return;
    }
    let Ok(Command { program, cwd, env }) = command(node, Some(home)) else { return };
    let args: Vec<std::ffi::OsString> = vec!["plugin".into(), "validate".into(), plugin.into()];
    match background::claude(&program, &args, &cwd, &env) {
        Ok(_) => {
            node.log.line(format_args!("Claude Code accepts the World's plugin, as {} reads it", home.display()));
            if let Err(error) =
                fastctx_platform::publish(&record, fingerprint.as_bytes(), fastctx_platform::Permissions::Private)
            {
                node.log.line(format_args!("keeping that Claude Code accepts the World's plugin: {error}"));
            }
        }
        Err(why) => node.log.line(format_args!(
            "Claude Code would not load the World's plugin, so its sessions tell the World nothing: {why}"
        )),
    }
}

/// The SHA-256 of every file of the plugin, by its path within it.
fn fingerprint(plugin: &Path) -> std::io::Result<String> {
    let mut files = Vec::new();
    let mut pending = vec![plugin.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory)? {
            let path = entry?.path();
            if path.is_dir() {
                pending.push(path);
            } else {
                files.push(path);
            }
        }
    }
    files.sort();
    let mut hash = Sha256::new();
    for file in files {
        let relative = file.strip_prefix(plugin).unwrap_or(&file);
        hash.update(relative.to_string_lossy().as_bytes());
        hash.update([0]);
        hash.update(std::fs::read(&file)?);
        hash.update([0]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
