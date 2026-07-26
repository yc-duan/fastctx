//! Shared project traversal with lossless search paths and deterministic failures.

use crate::bounded_sort::sort_cancelable;
use crate::file_executor::{BurstUse, GrepGlobExecutor};
use crate::operation::OperationCtx;
#[cfg(test)]
use crate::operation::TestStage;
use crate::path_codec::{
    PathRecord, ResolvedRoot, RootKind, display_path as search_display_path,
    io_error_message as search_io_error_message,
};
use crate::paths::ReadScope;
use globset::GlobSet;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use ignore::types::TypesBuilder;
use ignore::{DirEntry, Match as IgnoreMatch, WalkBuilder, WalkState};
use parking_lot::Mutex;
use std::fs;
use std::io::{self, Read};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub(crate) const TRAVERSAL_BATCH_ITEMS: usize = 256;
pub(crate) const MAX_IGNORE_CONFIG_BYTES: usize = 1024 * 1024;
const MAX_GIT_DIR_CONFIG_BYTES: u64 = 64 * 1024;
const OVERSIZED_IGNORE_CONFIG_ERROR: &str = "Ignore configuration exceeds maximum size.";

/// Legacy replace candidate retained while search uses `PathRecord` directly.
#[derive(Debug)]
pub(crate) struct ProjectCandidate {
    pub(crate) display: String,
}

/// The schedule-independent ordering key for one traversal failure.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct TraversalErrorKey {
    pub(crate) display_path_bytes: Vec<u8>,
    pub(crate) kind_rank: u8,
    pub(crate) raw_os_error: Option<i32>,
    pub(crate) normalized_message: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TraversalFailure {
    pub(crate) key: TraversalErrorKey,
    pub(crate) message: String,
}

/// Existing collection limit enforced at the first item beyond `maximum`.
#[derive(Clone, Copy)]
pub(crate) struct TraversalLimit {
    pub(crate) maximum: usize,
    pub(crate) message: &'static str,
}

/// Batched traversal output plus test-only evidence about lock and lane usage.
pub(crate) struct TraversalCollection<T> {
    pub(crate) items: Vec<T>,
    #[cfg(test)]
    pub(crate) metrics: TraversalMetrics,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct TraversalMetrics {
    pub(crate) serial_walks: usize,
    pub(crate) parallel_walks: usize,
    pub(crate) parallel_threads: usize,
    pub(crate) batch_lock_acquisitions: usize,
    pub(crate) largest_batch: usize,
}

impl TraversalFailure {
    pub(crate) fn from_io(path: &Path, error: &io::Error) -> Self {
        Self {
            key: TraversalErrorKey {
                display_path_bytes: search_display_path(path).into_bytes(),
                kind_rank: io_kind_rank(error.kind()),
                raw_os_error: error.raw_os_error(),
                normalized_message: normalize_error_message(&error.to_string()),
            },
            message: search_io_error_message(path, error),
        }
    }

    pub(crate) fn from_other(path: &Path, message: String) -> Self {
        Self {
            key: TraversalErrorKey {
                display_path_bytes: search_display_path(path).into_bytes(),
                kind_rank: u8::MAX,
                raw_os_error: None,
                normalized_message: normalize_error_message(&message),
            },
            message,
        }
    }
}

/// Collects lossless grep candidates while reusing the root's sole metadata result.
pub(crate) fn collect_search_candidates(
    root: &ResolvedRoot,
    glob: Option<&GlobSet>,
    file_type: Option<&str>,
    operation: Option<&OperationCtx>,
    executor: Option<&Arc<GrepGlobExecutor>>,
) -> Result<Vec<PathRecord>, String> {
    if operation_cancelled(operation) {
        return Err("Request cancelled.".to_string());
    }
    let type_filter = build_type_filter(file_type)?;
    if root.scope.is_restricted() {
        return collect_capability_candidates(root, glob, type_filter.as_ref(), operation);
    }
    let mut candidates = Vec::new();
    if root.kind == RootKind::File {
        let candidate =
            PathRecord::from_metadata(&root.native, root.match_root(), &root.metadata, true)
                .map_err(|error| search_io_error_message(&root.native, &error))?;
        if matches_record(&candidate, glob, type_filter.as_ref()) {
            candidates.push(candidate);
        }
    } else {
        candidates =
            collect_directory_candidates(root, glob, type_filter, operation, executor)?.items;
    }
    sort_cancelable(candidates, compare_search_candidates, operation, executor)
        .map(|sorted| sorted.items)
        .map_err(|error| error.to_string())
}

fn collect_capability_candidates(
    root: &ResolvedRoot,
    glob: Option<&GlobSet>,
    type_filter: Option<&ignore::types::Types>,
    operation: Option<&OperationCtx>,
) -> Result<Vec<PathRecord>, String> {
    collect_capability_candidates_filtered(
        root,
        glob,
        type_filter,
        operation,
        CapabilityCandidateOptions {
            honor_ignore: true,
            detail: CandidateDetail::Metadata,
            limit: None,
            limit_message: "",
        },
    )
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum CandidateDetail {
    Path,
    Metadata,
}

pub(crate) struct CapabilityCandidateOptions {
    pub(crate) honor_ignore: bool,
    pub(crate) detail: CandidateDetail,
    pub(crate) limit: Option<usize>,
    pub(crate) limit_message: &'static str,
}

pub(crate) fn collect_capability_candidates_filtered(
    root: &ResolvedRoot,
    glob: Option<&GlobSet>,
    type_filter: Option<&ignore::types::Types>,
    operation: Option<&OperationCtx>,
    options: CapabilityCandidateOptions,
) -> Result<Vec<PathRecord>, String> {
    let routed = root
        .scope
        .route_with_formatter(&root.native, search_display_path)?;
    if root.kind == RootKind::File {
        let candidate = if options.detail == CandidateDetail::Path {
            PathRecord::without_metadata(&root.native, root.match_root())
        } else {
            PathRecord::from_metadata(&root.native, root.match_root(), &root.metadata, true)
                .map_err(|error| search_io_error_message(&root.native, &error))?
        };
        return Ok(matches_record(&candidate, glob, type_filter)
            .then_some(candidate)
            .into_iter()
            .collect());
    }
    let mut candidates = Vec::new();
    let ignore = if options.honor_ignore {
        CapabilityIgnoreRules::at_root(&routed.capability, &routed.relative, &routed.canonical)?
    } else {
        CapabilityIgnoreRules::default()
    };
    let context = CapabilityTraversalContext {
        capability: &routed.capability,
        match_root: &root.native,
        scope: &root.scope,
        honor_ignore: options.honor_ignore,
        glob,
        type_filter,
        detail: options.detail,
        operation,
        limit: options.limit,
        limit_message: options.limit_message,
    };
    context.collect_directory(&routed.relative, &root.native, ignore, &mut candidates)?;
    sort_cancelable(candidates, compare_search_candidates, operation, None)
        .map(|sorted| sorted.items)
        .map_err(|error| error.to_string())
}

struct CapabilityTraversalContext<'a> {
    capability: &'a Arc<cap_std::fs::Dir>,
    match_root: &'a Path,
    scope: &'a ReadScope,
    honor_ignore: bool,
    glob: Option<&'a GlobSet>,
    type_filter: Option<&'a ignore::types::Types>,
    detail: CandidateDetail,
    operation: Option<&'a OperationCtx>,
    limit: Option<usize>,
    limit_message: &'static str,
}

impl CapabilityTraversalContext<'_> {
    fn collect_directory(
        &self,
        relative: &Path,
        native_dir: &Path,
        ignore: CapabilityIgnoreRules,
        candidates: &mut Vec<PathRecord>,
    ) -> Result<(), String> {
        // The capability walk is deliberately iterative: deeply nested trees must
        // not consume the process stack, and every child is discovered through the
        // startup capability rather than an ambient filesystem walk.
        let mut pending = vec![(relative.to_path_buf(), native_dir.to_path_buf(), ignore)];
        while let Some((relative, native_dir, ignore)) = pending.pop() {
            let entries = self
                .capability
                .read_dir(&relative)
                .map_err(|error| search_io_error_message(&native_dir, &error))?;
            for entry in entries {
                stage_traversal_entry(self.operation);
                if operation_cancelled(self.operation) {
                    return Err("Request cancelled.".to_string());
                }
                let entry = entry.map_err(|error| search_io_error_message(&native_dir, &error))?;
                let name = entry.file_name();
                let child_native = native_dir.join(&name);
                let child_relative = relative.join(&name);
                let file_type = entry
                    .file_type()
                    .map_err(|error| search_io_error_message(&child_native, &error))?;
                if file_type.is_dir() {
                    if name == ".git" && self.honor_ignore {
                        continue;
                    }
                    if self.honor_ignore && ignore.ignored(&child_native, true) {
                        // Match WalkBuilder's pruning behavior. A negation below an
                        // ignored directory is intentionally unreachable.
                        continue;
                    }
                    let child_ignore = if self.honor_ignore {
                        ignore.descend(self.capability, &child_relative, &child_native)?
                    } else {
                        CapabilityIgnoreRules::default()
                    };
                    pending.push((child_relative, child_native, child_ignore));
                    continue;
                }
                if !file_type.is_file() && !file_type.is_symlink() {
                    continue;
                }
                if self.honor_ignore && ignore.ignored(&child_native, false) {
                    continue;
                }
                let preliminary = PathRecord::without_metadata(&child_native, self.match_root);
                if !matches_record(&preliminary, self.glob, self.type_filter) {
                    continue;
                }
                let target = self
                    .scope
                    .route_with_formatter(&child_native, search_display_path)?;
                #[cfg(test)]
                if file_type.is_symlink() {
                    crate::file_snapshot::tests::notify_original_open(&child_native);
                }
                let metadata = if file_type.is_symlink() || self.detail == CandidateDetail::Metadata
                {
                    match target.capability.metadata(&target.relative) {
                        Ok(metadata) if metadata.is_file() => Some(metadata),
                        Ok(_) => continue,
                        Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                        Err(error) => return Err(search_io_error_message(&child_native, &error)),
                    }
                } else {
                    None
                };
                let confirmed = match self
                    .scope
                    .route_with_formatter(&child_native, search_display_path)
                {
                    Ok(confirmed) => confirmed,
                    Err(_) => continue,
                };
                if confirmed.canonical != target.canonical {
                    continue;
                }
                let candidate = match metadata {
                    Some(metadata) if self.detail == CandidateDetail::Metadata => {
                        PathRecord::from_metadata(&child_native, self.match_root, &metadata, true)
                            .map_err(|error| search_io_error_message(&child_native, &error))?
                    }
                    Some(_) | None => preliminary,
                };
                {
                    if self.limit.is_some_and(|limit| candidates.len() >= limit) {
                        return Err(self.limit_message.to_string());
                    }
                    candidates.push(candidate);
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Default)]
struct CapabilityIgnoreRules {
    global: MatcherStack,
    exclude: MatcherStack,
    gitignore: MatcherStack,
    ignore: MatcherStack,
    repository: Option<CapabilityRepository>,
}

type MatcherStack = Option<Arc<MatcherNode>>;

struct MatcherNode {
    matcher: Gitignore,
    parent: MatcherStack,
}

#[derive(Clone)]
struct CapabilityRepository {
    relative: std::path::PathBuf,
    native: std::path::PathBuf,
}

impl CapabilityIgnoreRules {
    fn at_root(
        capability: &Arc<cap_std::fs::Dir>,
        root_relative: &Path,
        root_native: &Path,
    ) -> Result<Self, String> {
        let (global, _) = Gitignore::global();
        let mut rules = Self::default();
        if !global.is_empty() {
            push_matcher(&mut rules.global, global);
        }
        // `WalkBuilder` loads parent ignore rules even when the request is
        // routed through a nested capability root. These configuration reads
        // are the one approved ambient exception: their matchers can only omit
        // candidates; traversal and every candidate operation stay capability
        // based.
        let mut ambient_ancestors = root_native.ancestors().skip(1).collect::<Vec<_>>();
        ambient_ancestors.reverse();
        for ancestor in ambient_ancestors {
            rules.add_ambient_directory(ancestor)?;
        }
        let mut native = root_native.to_path_buf();
        for component in root_relative.components() {
            if matches!(component, std::path::Component::Normal(_)) {
                native.pop();
            }
        }
        let mut ancestors = vec![(Path::new(".").to_path_buf(), native.clone())];
        let mut relative = Path::new(".").to_path_buf();
        for component in root_relative.components() {
            if !matches!(component, std::path::Component::Normal(_)) {
                continue;
            }
            relative.push(component.as_os_str());
            native.push(component.as_os_str());
            ancestors.push((relative.clone(), native.clone()));
        }

        for (relative, native) in &ancestors {
            if is_capability_repository(capability, relative)? {
                rules.repository = Some(CapabilityRepository {
                    relative: relative.clone(),
                    native: native.clone(),
                });
                rules.exclude = None;
                rules.gitignore = None;
            }
            if let Some(matcher) = read_capability_ignore(capability, relative, native, ".ignore")?
            {
                push_matcher(&mut rules.ignore, matcher);
            }
            if let Some(matcher) =
                read_capability_ignore(capability, relative, native, ".gitignore")?
            {
                push_matcher(&mut rules.gitignore, matcher);
            }
        }
        if let Some(repository) = &rules.repository
            && let Some(matcher) =
                read_capability_git_exclude(capability, &repository.relative, &repository.native)?
        {
            push_matcher(&mut rules.exclude, matcher);
        }
        Ok(rules)
    }

    fn descend(
        &self,
        capability: &Arc<cap_std::fs::Dir>,
        relative: &Path,
        native: &Path,
    ) -> Result<Self, String> {
        let mut rules = self.clone();
        if is_capability_repository(capability, relative)? {
            rules.repository = Some(CapabilityRepository {
                relative: relative.to_path_buf(),
                native: native.to_path_buf(),
            });
            rules.exclude = None;
            rules.gitignore = None;
            if let Some(matcher) = read_capability_git_exclude(capability, relative, native)? {
                push_matcher(&mut rules.exclude, matcher);
            }
        }
        if let Some(matcher) = read_capability_ignore(capability, relative, native, ".ignore")? {
            push_matcher(&mut rules.ignore, matcher);
        }
        if rules.repository.is_some()
            && let Some(matcher) =
                read_capability_ignore(capability, relative, native, ".gitignore")?
        {
            push_matcher(&mut rules.gitignore, matcher);
        }
        Ok(rules)
    }

    fn add_ambient_directory(&mut self, directory: &Path) -> Result<(), String> {
        if is_ambient_repository(directory)? {
            self.repository = Some(CapabilityRepository {
                relative: std::path::PathBuf::new(),
                native: directory.to_path_buf(),
            });
            self.exclude = None;
            self.gitignore = None;
            if let Some(matcher) = read_ambient_git_exclude(directory)? {
                push_matcher(&mut self.exclude, matcher);
            }
        }
        if let Some(matcher) = read_ambient_ignore(directory, ".ignore")? {
            push_matcher(&mut self.ignore, matcher);
        }
        // Git-related rules are collected from every ancestor, but only used
        // once a repository exists. This matches WalkBuilder's require-git
        // gating while retaining rules above the repository boundary.
        if let Some(matcher) = read_ambient_ignore(directory, ".gitignore")? {
            push_matcher(&mut self.gitignore, matcher);
        }
        Ok(())
    }

    fn ignored(&self, path: &Path, is_dir: bool) -> bool {
        // Ignore has four precedence tiers. Within each tier the deepest
        // directory wins; the tiers themselves are global < exclude <
        // .gitignore < .ignore, matching ignore::WalkBuilder project filters.
        let global = self
            .repository
            .as_ref()
            .and_then(|_| ignored_by_matchers(&self.global, path, is_dir));
        ignored_by_matchers(&self.ignore, path, is_dir)
            .or_else(|| ignored_by_matchers(&self.gitignore, path, is_dir))
            .or_else(|| ignored_by_matchers(&self.exclude, path, is_dir))
            .or(global)
            .unwrap_or(false)
    }
}

fn is_capability_repository(
    capability: &Arc<cap_std::fs::Dir>,
    relative: &Path,
) -> Result<bool, String> {
    match capability.metadata(relative.join(".git")) {
        // A worktree's `.git` is a gitdir file. It is still a repository
        // boundary even when its external common dir is outside the capability.
        Ok(metadata) => Ok(metadata.is_dir() || metadata.is_file()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(search_io_error_message(&relative.join(".git"), &error)),
    }
}

fn is_ambient_repository(directory: &Path) -> Result<bool, String> {
    match fs::metadata(directory.join(".git")) {
        Ok(metadata) => Ok(metadata.is_dir() || metadata.is_file()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(search_io_error_message(&directory.join(".git"), &error)),
    }
}

fn read_capability_ignore(
    capability: &Arc<cap_std::fs::Dir>,
    relative_dir: &Path,
    native_dir: &Path,
    name: &str,
) -> Result<Option<Gitignore>, String> {
    read_capability_ignore_path(
        capability,
        &relative_dir.join(name),
        native_dir,
        &native_dir.join(name),
    )
}

fn read_capability_git_exclude(
    capability: &Arc<cap_std::fs::Dir>,
    repository_relative: &Path,
    repository_native: &Path,
) -> Result<Option<Gitignore>, String> {
    let dot_git = repository_relative.join(".git");
    let source = repository_native.join(".git");
    let metadata = match capability.metadata(&dot_git) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(search_io_error_message(&source, &error)),
    };
    if metadata.is_dir() {
        return read_capability_ignore_path(
            capability,
            &dot_git.join("info/exclude"),
            repository_native,
            &source.join("info/exclude"),
        );
    }
    if !metadata.is_file() {
        return Ok(None);
    }
    let mut file = capability
        .open(&dot_git)
        .map_err(|error| search_io_error_message(&source, &error))?;
    let gitdir = read_first_line(&mut file, &source)?;
    let Some(gitdir) = gitdir.strip_prefix("gitdir: ") else {
        return Ok(None);
    };
    let common = resolve_worktree_common_dir(&source, gitdir)?;
    read_ambient_ignore_path(&common.join("info/exclude"), repository_native)
}

fn read_ambient_git_exclude(repository: &Path) -> Result<Option<Gitignore>, String> {
    let dot_git = repository.join(".git");
    let metadata = match fs::metadata(&dot_git) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(search_io_error_message(&dot_git, &error)),
    };
    if metadata.is_dir() {
        return read_ambient_ignore_path(&dot_git.join("info/exclude"), repository);
    }
    if !metadata.is_file() {
        return Ok(None);
    }
    let Some(gitdir) = read_ambient_first_line(&dot_git)? else {
        return Ok(None);
    };
    let Some(gitdir) = gitdir.strip_prefix("gitdir: ") else {
        return Ok(None);
    };
    let common = resolve_worktree_common_dir(&dot_git, gitdir)?;
    read_ambient_ignore_path(&common.join("info/exclude"), repository)
}

fn resolve_worktree_common_dir(dot_git: &Path, gitdir: &str) -> Result<std::path::PathBuf, String> {
    let gitdir = resolve_relative_config_path(dot_git.parent().unwrap_or(dot_git), gitdir);
    let common = read_ambient_first_line(&gitdir.join("commondir"))?
        .map(|commondir| resolve_relative_config_path(&gitdir, &commondir))
        .unwrap_or_else(|| gitdir.clone());
    Ok(common)
}

fn resolve_relative_config_path(base: &Path, value: &str) -> std::path::PathBuf {
    let path = Path::new(value);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

fn read_capability_ignore_path(
    capability: &Arc<cap_std::fs::Dir>,
    path: &Path,
    matcher_root: &Path,
    source: &Path,
) -> Result<Option<Gitignore>, String> {
    let file = match capability.open(path) {
        Ok(file) => file,
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(None);
        }
        Err(error) => return Err(search_io_error_message(source, &error)),
    };
    let mut file = file;
    let contents = read_bounded_ignore_config(&mut file, source)?;
    build_ignore(matcher_root, source, &contents)
}

fn read_ambient_ignore(directory: &Path, name: &str) -> Result<Option<Gitignore>, String> {
    read_ambient_ignore_path(&directory.join(name), directory)
}

fn read_ambient_ignore_path(
    source: &Path,
    matcher_root: &Path,
) -> Result<Option<Gitignore>, String> {
    let mut file = match fs::File::open(source) {
        Ok(file) => file,
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(None);
        }
        Err(error) => return Err(search_io_error_message(source, &error)),
    };
    let contents = read_bounded_ignore_config(&mut file, source)?;
    build_ambient_ignore(matcher_root, source, &contents)
}

fn read_first_line(file: &mut impl std::io::Read, source: &Path) -> Result<String, String> {
    let mut contents = String::new();
    file.take(MAX_GIT_DIR_CONFIG_BYTES)
        .read_to_string(&mut contents)
        .map_err(|error| search_io_error_message(source, &error))?;
    Ok(contents.lines().next().unwrap_or_default().to_string())
}

fn read_ambient_first_line(source: &Path) -> Result<Option<String>, String> {
    let mut file = match fs::File::open(source) {
        Ok(file) => file,
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(None);
        }
        Err(error) => return Err(search_io_error_message(source, &error)),
    };
    read_first_line(&mut file, source).map(Some)
}

fn read_bounded_ignore_config(
    file: &mut impl std::io::Read,
    source: &Path,
) -> Result<String, String> {
    let mut bytes = Vec::new();
    file.take((MAX_IGNORE_CONFIG_BYTES.saturating_add(1)) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| search_io_error_message(source, &error))?;
    if bytes.len() > MAX_IGNORE_CONFIG_BYTES {
        return Err(OVERSIZED_IGNORE_CONFIG_ERROR.to_string());
    }
    String::from_utf8(bytes).map_err(|error| {
        search_io_error_message(source, &io::Error::new(io::ErrorKind::InvalidData, error))
    })
}

fn build_ignore(
    matcher_root: &Path,
    source: &Path,
    contents: &str,
) -> Result<Option<Gitignore>, String> {
    let mut builder = GitignoreBuilder::new(matcher_root);
    for (index, line) in contents.lines().enumerate() {
        let line = if index == 0 {
            line.strip_prefix('\u{feff}').unwrap_or(line)
        } else {
            line
        };
        builder
            .add_line(Some(source.to_path_buf()), line)
            .map_err(|error| {
                format!(
                    "Cannot parse ignore file {}: {error}",
                    search_display_path(source)
                )
            })?;
    }
    builder.build().map(Some).map_err(|error| {
        format!(
            "Cannot parse ignore file {}: {error}",
            search_display_path(source)
        )
    })
}

fn build_ambient_ignore(
    matcher_root: &Path,
    source: &Path,
    contents: &str,
) -> Result<Option<Gitignore>, String> {
    let mut builder = GitignoreBuilder::new(matcher_root);
    for (index, line) in contents.lines().enumerate() {
        let line = if index == 0 {
            line.strip_prefix('\u{feff}').unwrap_or(line)
        } else {
            line
        };
        // Ambient configuration is allowed only to omit candidates. Ignore
        // malformed entries so an out-of-root pattern cannot surface in a
        // diagnostic, while valid rules in the same file still apply.
        let _ = builder.add_line(Some(source.to_path_buf()), line);
    }
    Ok(builder.build().ok())
}

fn push_matcher(stack: &mut MatcherStack, matcher: Gitignore) {
    *stack = Some(Arc::new(MatcherNode {
        matcher,
        parent: stack.clone(),
    }));
}

fn ignored_by_matchers(stack: &MatcherStack, path: &Path, is_dir: bool) -> Option<bool> {
    let mut stack = stack.as_deref();
    while let Some(node) = stack {
        let matched = node.matcher.matched(path, is_dir);
        match matched {
            IgnoreMatch::Ignore(_) => return Some(true),
            IgnoreMatch::Whitelist(_) => return Some(false),
            IgnoreMatch::None => stack = node.parent.as_deref(),
        }
    }
    None
}

/// Collects files for replace while preserving its pre-codec display contract.
pub(crate) fn collect_project_candidates(
    root: &Path,
    glob: Option<&GlobSet>,
    file_type: Option<&str>,
) -> Result<Vec<ProjectCandidate>, String> {
    let metadata =
        fs::metadata(root).map_err(|error| crate::paths::io_error_message(root, &error))?;
    let resolved = ResolvedRoot::from_metadata(root.to_path_buf(), metadata)?;
    collect_search_candidates(&resolved, glob, file_type, None, None).map(|candidates| {
        candidates
            .into_iter()
            .map(|candidate| ProjectCandidate {
                display: crate::paths::display_path(&candidate.native),
            })
            .collect()
    })
}

fn build_type_filter(file_type: Option<&str>) -> Result<Option<ignore::types::Types>, String> {
    let Some(file_type) = file_type else {
        return Ok(None);
    };
    let mut builder = TypesBuilder::new();
    builder.add_defaults();
    builder.select(file_type);
    builder.build().map(Some).map_err(|_| {
        format!(
            "Unknown file type: \"{file_type}\". Run with a glob filter instead, or use a standard type like js, py, rust, go, java."
        )
    })
}

fn collect_directory_candidates(
    root: &ResolvedRoot,
    glob: Option<&GlobSet>,
    type_filter: Option<ignore::types::Types>,
    operation: Option<&OperationCtx>,
    executor: Option<&Arc<GrepGlobExecutor>>,
) -> Result<TraversalCollection<PathRecord>, String> {
    let mut builder = WalkBuilder::new(&root.native);
    builder
        .hidden(false)
        .ignore(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .follow_links(false)
        .filter_entry(|entry| entry.depth() == 0 || entry.file_name() != ".git");
    if let Some(types) = type_filter {
        builder.types(types);
    }

    collect_walk_batched(builder, &root.native, operation, executor, None, |entry| {
        if !entry
            .file_type()
            .is_some_and(|file_type| file_type.is_file() || file_type.is_symlink())
        {
            return Ok(None);
        }
        let preliminary = PathRecord::without_metadata(entry.path(), &root.native);
        if !matches_record(&preliminary, glob, None) {
            return Ok(None);
        }
        candidate_from_entry(entry, &root.native)
    })
}

/// Runs a true serial walker when no traversal credit is immediately available;
/// parallel walkers merge only fixed-size thread-local batches.
pub(crate) fn collect_walk_batched<T, F>(
    mut builder: WalkBuilder,
    root: &Path,
    operation: Option<&OperationCtx>,
    executor: Option<&Arc<GrepGlobExecutor>>,
    limit: Option<TraversalLimit>,
    evaluate: F,
) -> Result<TraversalCollection<T>, String>
where
    T: Send,
    F: Fn(&DirEntry) -> Result<Option<T>, TraversalFailure> + Send + Sync,
{
    if operation_cancelled(operation) {
        return Err("Request cancelled.".to_string());
    }
    let permits = executor
        .map(|executor| executor.try_bursts(executor.extra_capacity(), BurstUse::TraversalExtra))
        .unwrap_or_default();
    if permits.is_empty() {
        return collect_walk_serial(builder, root, operation, limit, &evaluate);
    }

    let thread_count = permits.len().saturating_add(1);
    builder.threads(thread_count);
    let shared = Mutex::new(ParallelCollectionState::<T>::default());
    let stop = AtomicBool::new(false);
    let cancelled = AtomicBool::new(false);
    let evaluate = &evaluate;
    let run = catch_unwind(AssertUnwindSafe(|| {
        builder.build_parallel().run(|| {
            let mut local = ParallelLocalBatch::new(&shared, &stop, &cancelled, operation, limit);
            Box::new(move |entry| {
                process_parallel_entry(entry, root, operation, evaluate, &mut local)
            })
        });
    }));
    drop(permits);
    if cancelled.load(Ordering::Acquire) || operation_cancelled(operation) {
        return Err("Request cancelled.".to_string());
    }
    if run.is_err() {
        return Err("Internal traversal worker failure.".to_string());
    }
    finish_parallel_collection(shared.into_inner(), limit, thread_count)
}

fn collect_walk_serial<T, F>(
    builder: WalkBuilder,
    root: &Path,
    operation: Option<&OperationCtx>,
    limit: Option<TraversalLimit>,
    evaluate: &F,
) -> Result<TraversalCollection<T>, String>
where
    F: Fn(&DirEntry) -> Result<Option<T>, TraversalFailure>,
{
    let mut items = Vec::new();
    let mut minimum_failure = None;
    let mut too_many = false;
    for entry in builder.build() {
        stage_traversal_entry(operation);
        if operation_cancelled(operation) {
            return Err("Request cancelled.".to_string());
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                for failure in traversal_errors_from_ignore(&error, root) {
                    select_minimum_failure(&mut minimum_failure, failure);
                }
                continue;
            }
        };
        let evaluated = catch_unwind(AssertUnwindSafe(|| evaluate(&entry)));
        match evaluated {
            Ok(Ok(Some(item))) => {
                if limit.is_some_and(|limit| items.len() >= limit.maximum) {
                    too_many = true;
                    break;
                }
                items.push(item);
            }
            Ok(Ok(None)) => {}
            Ok(Err(failure)) => select_minimum_failure(&mut minimum_failure, failure),
            Err(_) => select_minimum_failure(
                &mut minimum_failure,
                TraversalFailure::from_other(
                    entry.path(),
                    "Internal traversal failure while evaluating a file candidate.".to_string(),
                ),
            ),
        }
    }
    if operation_cancelled(operation) {
        return Err("Request cancelled.".to_string());
    }
    if too_many {
        return match limit {
            Some(limit) => Err(limit.message.to_string()),
            None => Err("Internal traversal limit state was inconsistent.".to_string()),
        };
    }
    if let Some(failure) = minimum_failure {
        return Err(failure.message);
    }
    Ok(TraversalCollection {
        items,
        #[cfg(test)]
        metrics: TraversalMetrics {
            serial_walks: 1,
            ..TraversalMetrics::default()
        },
    })
}

struct ParallelCollectionState<T> {
    items: Vec<T>,
    minimum_failure: Option<TraversalFailure>,
    too_many: bool,
    #[cfg(test)]
    batch_lock_acquisitions: usize,
    #[cfg(test)]
    largest_batch: usize,
}

impl<T> Default for ParallelCollectionState<T> {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            minimum_failure: None,
            too_many: false,
            #[cfg(test)]
            batch_lock_acquisitions: 0,
            #[cfg(test)]
            largest_batch: 0,
        }
    }
}

struct ParallelLocalBatch<'a, T> {
    shared: &'a Mutex<ParallelCollectionState<T>>,
    stop: &'a AtomicBool,
    cancelled: &'a AtomicBool,
    operation: Option<&'a OperationCtx>,
    limit: Option<TraversalLimit>,
    items: Vec<T>,
    minimum_failure: Option<TraversalFailure>,
}

impl<'a, T> ParallelLocalBatch<'a, T> {
    fn new(
        shared: &'a Mutex<ParallelCollectionState<T>>,
        stop: &'a AtomicBool,
        cancelled: &'a AtomicBool,
        operation: Option<&'a OperationCtx>,
        limit: Option<TraversalLimit>,
    ) -> Self {
        Self {
            shared,
            stop,
            cancelled,
            operation,
            limit,
            items: Vec::with_capacity(TRAVERSAL_BATCH_ITEMS),
            minimum_failure: None,
        }
    }

    fn push(&mut self, item: T) {
        self.items.push(item);
        if self.items.len() == TRAVERSAL_BATCH_ITEMS {
            self.flush();
        }
    }

    fn record_failure(&mut self, failure: TraversalFailure) {
        select_minimum_failure(&mut self.minimum_failure, failure);
    }

    fn flush(&mut self) {
        if self.items.is_empty() && self.minimum_failure.is_none() {
            return;
        }
        stage_traversal_batch_flush(self.operation);
        if operation_cancelled(self.operation) {
            self.cancelled.store(true, Ordering::Release);
            self.stop.store(true, Ordering::Release);
            self.items.clear();
            self.minimum_failure = None;
            return;
        }

        let mut shared = self.shared.lock();
        #[cfg(test)]
        {
            let batch_len = self.items.len();
            shared.batch_lock_acquisitions = shared.batch_lock_acquisitions.saturating_add(1);
            shared.largest_batch = shared.largest_batch.max(batch_len);
        }
        if let Some(failure) = self.minimum_failure.take() {
            select_minimum_failure(&mut shared.minimum_failure, failure);
        }
        for item in self.items.drain(..) {
            if self
                .limit
                .is_some_and(|limit| shared.items.len() >= limit.maximum)
            {
                shared.too_many = true;
                self.stop.store(true, Ordering::Release);
                break;
            }
            shared.items.push(item);
        }
        drop(shared);
        stage_traversal_batch_flush(self.operation);
        if operation_cancelled(self.operation) {
            self.cancelled.store(true, Ordering::Release);
            self.stop.store(true, Ordering::Release);
        }
    }
}

impl<T> Drop for ParallelLocalBatch<'_, T> {
    fn drop(&mut self) {
        self.flush();
    }
}

fn process_parallel_entry<'a, T, F>(
    entry: Result<DirEntry, ignore::Error>,
    root: &Path,
    operation: Option<&OperationCtx>,
    evaluate: &F,
    local: &mut ParallelLocalBatch<'a, T>,
) -> WalkState
where
    F: Fn(&DirEntry) -> Result<Option<T>, TraversalFailure>,
{
    if local.stop.load(Ordering::Acquire) {
        return WalkState::Quit;
    }
    stage_traversal_entry(operation);
    if operation_cancelled(operation) {
        local.cancelled.store(true, Ordering::Release);
        local.stop.store(true, Ordering::Release);
        return WalkState::Quit;
    }
    let entry = match entry {
        Ok(entry) => entry,
        Err(error) => {
            for failure in traversal_errors_from_ignore(&error, root) {
                local.record_failure(failure);
            }
            return WalkState::Continue;
        }
    };
    let evaluated = catch_unwind(AssertUnwindSafe(|| evaluate(&entry)));
    match evaluated {
        Ok(Ok(Some(item))) => local.push(item),
        Ok(Ok(None)) => {}
        Ok(Err(failure)) => local.record_failure(failure),
        Err(_) => local.record_failure(TraversalFailure::from_other(
            entry.path(),
            "Internal traversal failure while evaluating a file candidate.".to_string(),
        )),
    }
    if local.stop.load(Ordering::Acquire) {
        WalkState::Quit
    } else {
        WalkState::Continue
    }
}

fn finish_parallel_collection<T>(
    state: ParallelCollectionState<T>,
    limit: Option<TraversalLimit>,
    _thread_count: usize,
) -> Result<TraversalCollection<T>, String> {
    if state.too_many {
        return match limit {
            Some(limit) => Err(limit.message.to_string()),
            None => Err("Internal traversal limit state was inconsistent.".to_string()),
        };
    }
    if let Some(failure) = state.minimum_failure {
        return Err(failure.message);
    }
    Ok(TraversalCollection {
        items: state.items,
        #[cfg(test)]
        metrics: TraversalMetrics {
            parallel_walks: 1,
            parallel_threads: _thread_count,
            batch_lock_acquisitions: state.batch_lock_acquisitions,
            largest_batch: state.largest_batch,
            ..TraversalMetrics::default()
        },
    })
}

fn compare_search_candidates(left: &PathRecord, right: &PathRecord) -> std::cmp::Ordering {
    right
        .modified
        .cmp(&left.modified)
        .then_with(|| left.display.as_bytes().cmp(right.display.as_bytes()))
        .then_with(|| left.native_key.cmp(&right.native_key))
}

fn matches_record(
    candidate: &PathRecord,
    glob: Option<&GlobSet>,
    types: Option<&ignore::types::Types>,
) -> bool {
    if let Some(types) = types
        && !types.matched(&candidate.native, false).is_whitelist()
    {
        return false;
    }
    glob.is_none_or(|glob| glob.is_match(candidate.relative_match.as_ref()))
}

fn candidate_from_path(
    path: &Path,
    match_root: &Path,
) -> Result<Option<PathRecord>, TraversalFailure> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => metadata,
        Ok(_) => return Ok(None),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(TraversalFailure::from_io(path, &error)),
    };
    candidate_from_metadata(path, match_root, &metadata).map(Some)
}

/// Symlinks follow their target for the regular-file check and ordering metadata.
fn candidate_from_entry(
    entry: &ignore::DirEntry,
    match_root: &Path,
) -> Result<Option<PathRecord>, TraversalFailure> {
    if entry
        .file_type()
        .is_some_and(|file_type| file_type.is_symlink())
    {
        return candidate_from_path(entry.path(), match_root);
    }
    match entry.metadata() {
        Ok(metadata) if metadata.is_file() => {
            candidate_from_metadata(entry.path(), match_root, &metadata).map(Some)
        }
        Ok(_) => Ok(None),
        Err(_) => candidate_from_path(entry.path(), match_root),
    }
}

fn candidate_from_metadata(
    path: &Path,
    match_root: &Path,
    metadata: &fs::Metadata,
) -> Result<PathRecord, TraversalFailure> {
    PathRecord::from_metadata(path, match_root, metadata, true)
        .map_err(|error| TraversalFailure::from_io(path, &error))
}

fn operation_cancelled(operation: Option<&OperationCtx>) -> bool {
    operation.is_some_and(|operation| operation.check().is_err())
}

fn stage_traversal_entry(operation: Option<&OperationCtx>) {
    #[cfg(test)]
    if let Some(operation) = operation {
        operation.stage(TestStage::TraversalEntry);
    }
    #[cfg(not(test))]
    let _ = operation;
}

fn stage_traversal_batch_flush(operation: Option<&OperationCtx>) {
    #[cfg(test)]
    if let Some(operation) = operation {
        operation.stage(TestStage::TraversalBatchFlush);
    }
    #[cfg(not(test))]
    let _ = operation;
}

fn select_minimum_failure(current: &mut Option<TraversalFailure>, failure: TraversalFailure) {
    if current
        .as_ref()
        .is_none_or(|existing| failure.key < existing.key)
    {
        *current = Some(failure);
    }
}

pub(crate) fn traversal_errors_from_ignore(
    error: &ignore::Error,
    root: &Path,
) -> Vec<TraversalFailure> {
    let mut failures = Vec::new();
    collect_ignore_error(error, None, root, &mut failures);
    failures
}

fn collect_ignore_error(
    error: &ignore::Error,
    inherited_path: Option<&Path>,
    root: &Path,
    failures: &mut Vec<TraversalFailure>,
) {
    match error {
        ignore::Error::Partial(errors) => {
            for error in errors {
                collect_ignore_error(error, inherited_path, root, failures);
            }
        }
        ignore::Error::WithLineNumber { err, .. } | ignore::Error::WithDepth { err, .. } => {
            collect_ignore_error(err, inherited_path, root, failures);
        }
        ignore::Error::WithPath { path, err } => {
            collect_ignore_error(err, Some(path), root, failures);
        }
        ignore::Error::Loop { child, .. } => failures.push(TraversalFailure::from_other(
            child,
            format!("Cannot traverse path: {error}"),
        )),
        ignore::Error::Io(error) => failures.push(TraversalFailure::from_io(
            inherited_path.unwrap_or(root),
            error,
        )),
        ignore::Error::Glob { .. }
        | ignore::Error::UnrecognizedFileType(_)
        | ignore::Error::InvalidDefinition => failures.push(TraversalFailure::from_other(
            inherited_path.unwrap_or(root),
            format!("Cannot traverse path: {error}"),
        )),
    }
}

fn normalize_error_message(message: &str) -> String {
    message.replace("\r\n", "\n").replace('\r', "\n")
}

fn io_kind_rank(kind: io::ErrorKind) -> u8 {
    match kind {
        io::ErrorKind::NotFound => 0,
        io::ErrorKind::PermissionDenied => 1,
        io::ErrorKind::WouldBlock => 2,
        io::ErrorKind::TimedOut => 3,
        io::ErrorKind::Interrupted => 4,
        io::ErrorKind::InvalidInput | io::ErrorKind::InvalidData => 5,
        io::ErrorKind::UnexpectedEof => 6,
        _ => 254,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        TRAVERSAL_BATCH_ITEMS, TraversalCollection, TraversalFailure, collect_walk_batched,
        traversal_errors_from_ignore,
    };
    use crate::file_executor::{BurstUse, GrepGlobExecutor};
    use crate::operation::{RequestWorkGuard, TestStage};
    use ignore::WalkBuilder;
    use rmcp::model::RequestId;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio_util::sync::CancellationToken;

    fn unfiltered_builder(root: &Path) -> WalkBuilder {
        let mut builder = WalkBuilder::new(root);
        builder
            .standard_filters(false)
            .hidden(false)
            .follow_links(false);
        builder
    }

    fn collect_file_names(
        root: &Path,
        executor: &Arc<GrepGlobExecutor>,
        operation: Option<&crate::operation::OperationCtx>,
    ) -> Result<TraversalCollection<String>, String> {
        collect_walk_batched(
            unfiltered_builder(root),
            root,
            operation,
            Some(executor),
            None,
            |entry| {
                if entry.file_type().is_some_and(|kind| kind.is_file()) {
                    Ok(Some(entry.path().to_string_lossy().into_owned()))
                } else {
                    Ok(None)
                }
            },
        )
    }

    fn create_batched_fixture() -> tempfile::TempDir {
        let fixture = tempfile::tempdir().unwrap();
        for directory_index in 0..8 {
            let directory = fixture.path().join(format!("batch-{directory_index:02}"));
            fs::create_dir(&directory).unwrap();
            for file_index in 0..137 {
                fs::write(directory.join(format!("item-{file_index:03}.txt")), b"x").unwrap();
            }
        }
        fixture
    }

    #[test]
    fn nested_ignore_error_keeps_the_path_in_its_canonical_key() {
        let error = ignore::Error::WithDepth {
            depth: 2,
            err: Box::new(ignore::Error::WithPath {
                path: PathBuf::from("nested/private"),
                err: Box::new(ignore::Error::Io(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "denied",
                ))),
            }),
        };
        let failures = traversal_errors_from_ignore(&error, Path::new("root"));
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].key.display_path_bytes, b"nested/private");
        assert_eq!(failures[0].message, "Permission denied: nested/private");
    }

    #[test]
    fn traversal_failure_reduction_is_schedule_independent() {
        let fixture = tempfile::tempdir().unwrap();
        for index in 0..8 {
            let directory = fixture.path().join(format!("worker-{index}"));
            fs::create_dir(&directory).unwrap();
            let name = match index % 3 {
                0 => "first-other",
                1 => "first-denied",
                _ => "last-denied",
            };
            fs::write(directory.join(name), b"x").unwrap();
        }

        for parallelism in [1, 2, 4] {
            let executor = Arc::new(GrepGlobExecutor::with_test_parallelism(parallelism));
            for _ in 0..100 {
                let result = collect_walk_batched(
                    unfiltered_builder(fixture.path()),
                    fixture.path(),
                    None,
                    Some(&executor),
                    None,
                    |entry| {
                        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
                            return Ok(None::<()>);
                        }
                        let (path, kind, message) = match entry.file_name().to_str() {
                            Some("first-other") => ("a-first", std::io::ErrorKind::Other, "other"),
                            Some("first-denied") => {
                                ("a-first", std::io::ErrorKind::PermissionDenied, "denied")
                            }
                            _ => ("z-last", std::io::ErrorKind::PermissionDenied, "z"),
                        };
                        let error = std::io::Error::new(kind, message);
                        Err(TraversalFailure::from_io(Path::new(path), &error))
                    },
                );
                assert_eq!(
                    result
                        .err()
                        .expect("every file injects a traversal failure"),
                    "Permission denied: a-first"
                );
            }
            executor.wait_for_test_quiescence();
            let ledger = executor.test_burst_ledger();
            assert_eq!(ledger.allocated, ledger.released);
            assert_eq!(ledger.live, 0);
            assert_eq!(ledger.duplicate_releases, 0);
        }
    }

    #[test]
    fn p1_parallel_and_saturated_p4_have_the_same_set_and_true_serial_fallback() {
        let fixture = create_batched_fixture();
        let expected_count = 8 * 137;

        let p1 = Arc::new(GrepGlobExecutor::with_test_parallelism(1));
        let mut serial = collect_file_names(fixture.path(), &p1, None).unwrap();
        assert_eq!(serial.items.len(), expected_count);
        assert_eq!(serial.metrics.serial_walks, 1);
        assert_eq!(serial.metrics.parallel_walks, 0);

        let p4 = Arc::new(GrepGlobExecutor::with_test_parallelism(4));
        let mut parallel = collect_file_names(fixture.path(), &p4, None).unwrap();
        assert_eq!(parallel.items.len(), expected_count);
        assert_eq!(parallel.metrics.serial_walks, 0);
        assert_eq!(parallel.metrics.parallel_walks, 1);
        assert_eq!(parallel.metrics.parallel_threads, 4);
        assert!(parallel.metrics.largest_batch <= TRAVERSAL_BATCH_ITEMS);
        assert!(
            parallel.metrics.batch_lock_acquisitions
                <= expected_count.div_ceil(TRAVERSAL_BATCH_ITEMS)
                    + parallel.metrics.parallel_threads
        );

        serial.items.sort();
        parallel.items.sort();
        assert_eq!(parallel.items, serial.items);

        let held = p4.try_bursts(p4.extra_capacity(), BurstUse::SearchSpeculation);
        assert_eq!(held.len(), p4.extra_capacity());
        let mut saturated = collect_file_names(fixture.path(), &p4, None).unwrap();
        assert_eq!(saturated.metrics.serial_walks, 1);
        assert_eq!(saturated.metrics.parallel_walks, 0);
        saturated.items.sort();
        assert_eq!(saturated.items, serial.items);
        drop(held);

        p4.wait_for_test_quiescence();
        let ledger = p4.test_burst_ledger();
        assert_eq!(ledger.allocated, ledger.released);
        assert_eq!(ledger.live, 0);
        assert_eq!(ledger.duplicate_releases, 0);
    }

    #[test]
    fn traversal_entry_and_batch_flush_cancellation_release_every_walk_credit() {
        let fixture = create_batched_fixture();
        for target in [TestStage::TraversalEntry, TestStage::TraversalBatchFlush] {
            let parent = CancellationToken::new();
            let cancel = parent.clone();
            let fired = Arc::new(AtomicBool::new(false));
            let fired_hook = Arc::clone(&fired);
            let (mut guard, operation) = RequestWorkGuard::new_with_hook(
                RequestId::String(Arc::from(format!("traversal-{target:?}"))),
                parent,
                Arc::new(move |stage| {
                    if stage == target && !fired_hook.swap(true, Ordering::AcqRel) {
                        cancel.cancel();
                    }
                }),
            );
            let executor = Arc::new(GrepGlobExecutor::with_test_parallelism(4));
            let error = collect_file_names(fixture.path(), &executor, Some(&operation))
                .err()
                .expect("the selected traversal stage must cancel the collection");
            assert_eq!(error, "Request cancelled.");
            assert!(fired.load(Ordering::Acquire));
            guard.disarm();
            executor.wait_for_test_quiescence();
            let ledger = executor.test_burst_ledger();
            assert_eq!(ledger.allocated, ledger.released);
            assert_eq!(ledger.live, 0);
            assert_eq!(ledger.duplicate_releases, 0);
        }
    }
}
