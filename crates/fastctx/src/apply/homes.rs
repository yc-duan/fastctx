//! Command-line target groups: directories belong to the preceding target,
//! and a target without directories selects that host's default home.

use std::path::PathBuf;

use clap::{ArgMatches, Args, Command, FromArgMatches};
use fastctx_install::apply::{Host, Hosts, Places};

use super::Target;

#[derive(Args)]
struct Raw {
    /// Select this host; repeat to select more. Without it, installed hosts.
    #[arg(long, value_enum)]
    target: Vec<Target>,
    /// A directory for the preceding --target; repeat for several homes.
    /// A target without --home uses CODEX_HOME / CLAUDE_CONFIG_DIR or ~/.codex / ~/.claude.
    #[arg(long, requires = "target", value_name = "DIRECTORY", value_parser = crate::paths::parser())]
    home: Vec<PathBuf>,
}

pub(super) struct Homes {
    selected: Vec<(Target, Option<PathBuf>)>,
}

impl Args for Homes {
    fn augment_args(command: Command) -> Command {
        Raw::augment_args(command)
    }

    fn augment_args_for_update(command: Command) -> Command {
        Raw::augment_args_for_update(command)
    }
}

impl FromArgMatches for Homes {
    fn from_arg_matches(matches: &ArgMatches) -> Result<Self, clap::Error> {
        enum Event {
            Target(Target),
            Home(PathBuf),
        }
        let raw = Raw::from_arg_matches(matches)?;
        let mut events = Vec::new();
        if let Some(indices) = matches.indices_of("target") {
            events.extend(indices.zip(raw.target).map(|(at, target)| (at, Event::Target(target))));
        }
        if let Some(indices) = matches.indices_of("home") {
            events.extend(indices.zip(raw.home).map(|(at, home)| (at, Event::Home(home))));
        }
        events.sort_by_key(|(at, _)| *at);
        let mut selected = Vec::new();
        let mut group = None;
        let mut has_home = false;
        for (_, event) in events {
            match event {
                Event::Target(target) => {
                    if let Some(target) = group
                        && !has_home
                    {
                        selected.push((target, None));
                    }
                    group = Some(target);
                    has_home = false;
                }
                Event::Home(home) => {
                    let target = group.ok_or_else(|| {
                        clap::Error::raw(clap::error::ErrorKind::InvalidValue, "--home must follow its --target")
                    })?;
                    selected.push((target, Some(home)));
                    has_home = true;
                }
            }
        }
        if let Some(target) = group
            && !has_home
        {
            selected.push((target, None));
        }
        Ok(Self { selected })
    }

    fn update_from_arg_matches(&mut self, matches: &ArgMatches) -> Result<(), clap::Error> {
        *self = Self::from_arg_matches(matches)?;
        Ok(())
    }
}

impl Homes {
    pub(super) fn places(&self, defaults: &Places, into: bool) -> anyhow::Result<Vec<(Places, Hosts)>> {
        if self.selected.is_empty() {
            return Ok(vec![(defaults.clone(), super::hosts(defaults, into)?)]);
        }
        let mut selected = Vec::new();
        let mut identities = Vec::new();
        for (target, home) in &self.selected {
            let (host, default) = match target {
                Target::Codex => (Host::Codex, &defaults.codex_home),
                Target::Claude => (Host::Claude, &defaults.claude_home),
            };
            let places = defaults.at_home(host, home.as_ref().unwrap_or(default))?;
            let home = match host {
                Host::Codex => &places.codex_home,
                Host::Claude => &places.claude_home,
            };
            if identities.contains(&(host, home.clone())) {
                continue;
            }
            identities.push((host, home.clone()));
            selected.push((places, Hosts { codex: host == Host::Codex, claude: host == Host::Claude }));
        }
        Ok(selected)
    }
}
