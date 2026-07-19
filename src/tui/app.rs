//! Pure TUI state transitions and controlled I/O effects.

use super::config::{ConfigCursor, ConfigDraft, ConfigViewport};
use super::jobs::{JobsDetail, JobsState, JobsViewport, visible_job_count, visible_jobs};
use super::update::{self as update_copy, UpdateMessages};
use crate::control::apply::{
    ApplyOptions, ApplyPlan, OperationReceipt, UnapplyOptions, UnapplyPlan, commit_apply,
    commit_unapply, plan_apply, plan_unapply,
};
use crate::control::doctor::{self, DoctorReport};
use crate::control::i18n::{ALL_LANGUAGES, Language, Messages};
use crate::control::job_i18n::{self, JobMessages};
use crate::control::paths::ControlPaths;
use crate::control::settings::{self, FastCtxSettings};
use crate::shell::jobs::{self, JobSummary};
use crate::update::{CheckFailure, CheckFailureKind, StartupUpdate, UpdatePlan};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

const JOB_LIST_REFRESH: Duration = Duration::from_secs(1);
const JOB_TAIL_REFRESH: Duration = Duration::from_millis(300);
const JOB_TAIL_LINES: usize = 512;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Screen {
    Update,
    UpdateChecking,
    UpdateConfirm,
    Language { first_run: bool },
    Main,
    ApplyHome,
    ApplyLoading,
    ApplyPreview,
    ApplyConflict,
    ApplyConfirm,
    ApplyRunning,
    UnapplyLoading,
    UnapplyPreview,
    UnapplyConfirm,
    UnapplyRunning,
    Config,
    Jobs,
    JobsKillConfirm,
    JobsKilling,
    JobsKillFailed,
    Status,
    About,
    Receipt,
    OperationFailed,
}

#[derive(Clone, Debug)]
pub(crate) enum StatusState {
    Loading,
    Ready(DoctorReport),
    Empty,
    Error(String),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct DetailViewport {
    screen: Option<Screen>,
    offset: usize,
    maximum_offset: usize,
    page_size: usize,
}

impl DetailViewport {
    pub(crate) fn enter(&mut self, screen: Screen) {
        if self.screen != Some(screen) {
            self.screen = Some(screen);
            self.offset = 0;
            self.maximum_offset = 0;
            self.page_size = 0;
        }
    }

    pub(crate) fn update(&mut self, total_rows: usize, visible_rows: usize) {
        self.page_size = visible_rows;
        self.maximum_offset = total_rows.saturating_sub(visible_rows);
        self.offset = self.offset.min(self.maximum_offset);
    }

    pub(crate) const fn offset(self) -> usize {
        self.offset
    }

    pub(crate) const fn can_move_up(self) -> bool {
        self.offset > 0
    }

    pub(crate) const fn can_move_down(self) -> bool {
        self.offset < self.maximum_offset
    }

    fn handle_key(&mut self, key: KeyCode) -> bool {
        match key {
            KeyCode::Up | KeyCode::Char('k') => {
                self.offset = self.offset.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.offset = self.offset.saturating_add(1).min(self.maximum_offset);
            }
            KeyCode::PageUp => {
                self.offset = self.offset.saturating_sub(self.page_size.max(1));
            }
            KeyCode::PageDown => {
                self.offset = self
                    .offset
                    .saturating_add(self.page_size.max(1))
                    .min(self.maximum_offset);
            }
            KeyCode::Home => self.offset = 0,
            KeyCode::End => self.offset = self.maximum_offset,
            _ => return false,
        }
        true
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Toast {
    pub(crate) message: String,
    pub(crate) warning: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Effect {
    RetryUpdate,
    SaveLanguage { first_run: bool },
    SaveConfig,
    PlanApply,
    CommitApply,
    PlanUnapply,
    CommitUnapply,
    RunDoctor,
    LoadJobs,
    LoadJobTail { job_id: String },
    RefreshJobCount,
    KillJob { job_id: String },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum UpdateCheckPurpose {
    Startup,
    UpdatePage,
}

pub(crate) struct App {
    pub paths: ControlPaths,
    pub settings: FastCtxSettings,
    pub language: Language,
    pub screen: Screen,
    pub selected: usize,
    pub config_draft: ConfigDraft,
    pub config_cursor: ConfigCursor,
    pub config_viewport: ConfigViewport,
    pub jobs_state: JobsState,
    pub jobs_detail: JobsDetail,
    pub jobs_selected: usize,
    pub jobs_viewport: JobsViewport,
    pub(crate) detail_viewport: DetailViewport,
    pub pending_job: Option<JobSummary>,
    pub running_job_count: Option<usize>,
    pub status: StatusState,
    pub receipt: Option<OperationReceipt>,
    pub error: Option<String>,
    pub toast: Option<Toast>,
    pub should_quit: bool,
    pub(crate) update_state: StartupUpdate,
    current_executable: PathBuf,
    exit_update: Option<UpdatePlan>,
    pub(crate) apply_plan: Option<ApplyPlan>,
    pub(crate) unapply_plan: Option<UnapplyPlan>,
    pending: Option<Effect>,
    retry_effect: Option<Effect>,
    last_jobs_refresh: Option<Instant>,
    last_tail_refresh: Option<Instant>,
    update_check: Option<(UpdateCheckPurpose, Receiver<StartupUpdate>)>,
}

impl App {
    #[cfg(test)]
    pub fn load(paths: ControlPaths) -> Result<Self, String> {
        Self::load_with_startup(paths, StartupUpdate::None, None)
    }

    pub(crate) fn load_with_startup(
        paths: ControlPaths,
        startup_update: StartupUpdate,
        startup_notice: Option<crate::update::FinalizeNotice>,
    ) -> Result<Self, String> {
        let settings = settings::load(&paths)?;
        let running_job_count = jobs::running_summaries(&paths)
            .ok()
            .map(|running| running.len());
        let language = settings
            .language
            .as_deref()
            .and_then(Language::parse)
            .unwrap_or_else(Language::detect);
        let home_screen = if settings.language.is_some() {
            Screen::Main
        } else {
            Screen::Language { first_run: true }
        };
        let screen = home_screen;
        let selected = if matches!(screen, Screen::Language { .. }) {
            language_index(language)
        } else {
            0
        };
        let notice_language = if settings.language.is_some() {
            language
        } else {
            Language::En
        };
        let startup_notice = startup_notice.map(|notice| {
            let messages = update_copy::messages(notice_language);
            match notice.outcome {
                crate::update::FinalizeOutcome::Updated => Toast {
                    message: messages.updated.replace("{version}", &notice.version),
                    warning: false,
                },
                crate::update::FinalizeOutcome::RuntimeUpdated => Toast {
                    message: messages
                        .updated_runtime
                        .replace("{version}", &notice.version),
                    warning: false,
                },
                crate::update::FinalizeOutcome::RuntimeUnchanged(detail) => Toast {
                    message: format!(
                        "{}: {detail}",
                        messages
                            .runtime_unchanged
                            .replace("{version}", &notice.version)
                    ),
                    warning: true,
                },
            }
        });
        let startup_failure = match &startup_update {
            StartupUpdate::Available(_) => Some(Toast {
                message: update_copy::messages(notice_language)
                    .available_title
                    .to_string(),
                warning: false,
            }),
            StartupUpdate::NpmPending { .. } => Some(Toast {
                message: update_copy::messages(notice_language)
                    .pending_title
                    .to_string(),
                warning: false,
            }),
            StartupUpdate::Failed(error) if error.kind == CheckFailureKind::Structural => {
                Some(Toast {
                    message: format!(
                        "{}: {}",
                        update_copy::messages(if settings.language.is_some() {
                            language
                        } else {
                            Language::En
                        })
                        .check_failed,
                        error.message
                    ),
                    warning: true,
                })
            }
            StartupUpdate::InstallFailed(error) => Some(Toast {
                message: format!(
                    "{}: {error}",
                    update_copy::messages(if settings.language.is_some() {
                        language
                    } else {
                        Language::En
                    })
                    .update_failed
                ),
                warning: true,
            }),
            StartupUpdate::None | StartupUpdate::NpmCurrent { .. } | StartupUpdate::Failed(_) => {
                None
            }
        };
        Ok(Self {
            config_draft: ConfigDraft::from_settings(&settings),
            config_cursor: ConfigCursor::default(),
            config_viewport: ConfigViewport::default(),
            jobs_state: JobsState::Loading,
            jobs_detail: JobsDetail::default(),
            jobs_selected: 0,
            jobs_viewport: JobsViewport::default(),
            detail_viewport: DetailViewport::default(),
            pending_job: None,
            running_job_count,
            paths,
            settings,
            language,
            screen,
            selected,
            status: StatusState::Loading,
            receipt: None,
            error: None,
            toast: startup_notice.or(startup_failure),
            should_quit: false,
            update_state: startup_update,
            current_executable: std::env::current_exe()
                .map_err(|error| format!("Cannot locate the running fastctx binary: {error}"))?,
            exit_update: None,
            apply_plan: None,
            unapply_plan: None,
            pending: None,
            retry_effect: None,
            last_jobs_refresh: Some(Instant::now()),
            last_tail_refresh: None,
            update_check: None,
        })
    }

    #[cfg(test)]
    pub(crate) fn for_test(paths: ControlPaths, executable: PathBuf) -> Self {
        let mut app = Self::load(paths).unwrap();
        app.current_executable = executable;
        app
    }

    pub fn messages(&self) -> &'static Messages {
        // Locale may preselect a row, but the UI stays English until the user chooses a language.
        if self.settings.language.is_none() {
            Language::En.messages()
        } else {
            self.language.messages()
        }
    }

    pub(crate) fn unapply_processes_message(&self) -> &'static str {
        let language = if self.settings.language.is_none() {
            Language::En
        } else {
            self.language
        };
        crate::control::i18n::unapply_processes_message(language)
    }

    pub fn job_messages(&self) -> &'static JobMessages {
        let language = if self.settings.language.is_none() {
            Language::En
        } else {
            self.language
        };
        job_i18n::messages(language)
    }

    pub(crate) fn update_messages(&self) -> &'static UpdateMessages {
        let language = if self.settings.language.is_none() {
            Language::En
        } else {
            self.language
        };
        update_copy::messages(language)
    }

    pub(crate) fn take_update_plan(&mut self) -> Option<UpdatePlan> {
        self.exit_update.take()
    }

    pub(crate) fn set_startup_update_check(&mut self, receiver: Receiver<StartupUpdate>) {
        self.update_check = Some((UpdateCheckPurpose::Startup, receiver));
    }

    pub(crate) fn poll_update_check(&mut self) {
        let Some((purpose, receiver)) = self.update_check.as_ref() else {
            return;
        };
        let purpose = *purpose;
        let result = match receiver.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(StartupUpdate::Failed(CheckFailure {
                kind: CheckFailureKind::Structural,
                message: "the update-check worker stopped without a result".to_string(),
            })),
        };
        if let Some(result) = result {
            self.update_check = None;
            self.resolve_update_check(purpose, result);
        }
    }

    pub fn has_pending_effect(&self) -> bool {
        self.pending.is_some()
    }

    pub fn tick(&mut self) {
        if self.pending.is_some() {
            return;
        }
        let now = Instant::now();
        match self.screen {
            Screen::Main
                if self
                    .last_jobs_refresh
                    .is_none_or(|last| now.duration_since(last) >= JOB_LIST_REFRESH) =>
            {
                self.pending = Some(Effect::RefreshJobCount);
            }
            Screen::Jobs => {
                if matches!(self.jobs_state, JobsState::Loading)
                    || self
                        .last_jobs_refresh
                        .is_none_or(|last| now.duration_since(last) >= JOB_LIST_REFRESH)
                {
                    self.pending = Some(Effect::LoadJobs);
                } else if let Some(job_id) = self.focused_job().map(|job| job.id.clone())
                    && (self.jobs_detail.job_id.as_deref() != Some(job_id.as_str())
                        || self
                            .last_tail_refresh
                            .is_none_or(|last| now.duration_since(last) >= JOB_TAIL_REFRESH))
                {
                    self.pending = Some(Effect::LoadJobTail { job_id });
                }
            }
            _ => {}
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        self.toast = None;
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }
        match self.screen {
            Screen::Update => self.handle_update(key.code),
            Screen::UpdateConfirm => self.handle_update_confirm(key.code),
            Screen::Language { first_run } => self.handle_language(key.code, first_run),
            Screen::Main => self.handle_main(key.code),
            Screen::ApplyHome => self.handle_apply_home(key.code),
            Screen::ApplyPreview => self.handle_apply_preview(key.code),
            Screen::ApplyConflict => {
                self.handle_binary_confirmation(key.code, Screen::ApplyConfirm)
            }
            Screen::ApplyConfirm => self.handle_apply_confirm(key.code),
            Screen::UnapplyPreview => self.handle_unapply_preview(key.code),
            Screen::UnapplyConfirm => self.handle_unapply_confirm(key.code),
            Screen::Config => self.handle_config(key),
            Screen::Jobs => self.handle_jobs(key.code),
            Screen::JobsKillConfirm => self.handle_jobs_kill_confirm(key.code),
            Screen::JobsKillFailed => self.handle_jobs_kill_failed(key.code),
            Screen::Status => self.handle_status(key.code),
            Screen::About => self.handle_simple_child(key.code),
            Screen::Receipt => self.handle_receipt(key.code),
            Screen::OperationFailed => self.handle_operation_failed(key.code),
            Screen::UpdateChecking
            | Screen::ApplyLoading
            | Screen::ApplyRunning
            | Screen::UnapplyLoading
            | Screen::UnapplyRunning
            | Screen::JobsKilling => {}
        }
    }

    pub fn execute_pending(&mut self) {
        let Some(effect) = self.pending.take() else {
            return;
        };
        let retry_effect = match &effect {
            Effect::CommitApply => Effect::PlanApply,
            Effect::CommitUnapply => Effect::PlanUnapply,
            effect => effect.clone(),
        };
        let is_doctor_effect = matches!(&effect, Effect::RunDoctor);
        let is_kill_effect = matches!(&effect, Effect::KillJob { .. });
        let result = match effect {
            Effect::RetryUpdate => {
                self.update_check = Some((
                    UpdateCheckPurpose::UpdatePage,
                    crate::update::spawn_update_check(self.paths.clone(), true),
                ));
                Ok(())
            }
            Effect::SaveLanguage { first_run: _ } => {
                let mut updated = self.settings.clone();
                updated.language = Some(self.language.code().to_string());
                settings::save(&self.paths, &updated).map(|_| {
                    self.settings = updated;
                    self.screen = Screen::Main;
                    self.selected = 0;
                })
            }
            Effect::SaveConfig => {
                let mut updated = self.settings.clone();
                self.config_draft.apply_to(&mut updated);
                let extensions_changed =
                    updated.fastshell.enabled != self.settings.fastshell.enabled;
                let limits_changed = updated.fastshell.job_storage_limit_mib
                    != self.settings.fastshell.job_storage_limit_mib
                    || updated.fastshell.max_running_jobs
                        != self.settings.fastshell.max_running_jobs
                    || updated.fastshell.job_list_limit != self.settings.fastshell.job_list_limit;
                settings::save(&self.paths, &updated).map(|_| {
                    self.settings = updated;
                    self.back_to_main();
                    let mut message = vec![self.messages().settings_saved];
                    if extensions_changed {
                        message.push(self.messages().extensions_note);
                    }
                    if limits_changed {
                        message.push(self.job_messages().user_limit_note);
                    }
                    self.toast = Some(Toast {
                        message: message.join("\n"),
                        warning: false,
                    });
                })
            }
            Effect::PlanApply => plan_apply(
                &self.paths,
                ApplyOptions {
                    tier: self.settings.tier,
                    tool_budgets: self.settings.tool_budgets,
                    fastshell_enabled: self.settings.fastshell.enabled,
                    current_executable: self.current_executable.clone(),
                },
            )
            .map(|plan| {
                self.apply_plan = Some(plan);
                self.screen = Screen::ApplyPreview;
                self.selected = 0;
            }),
            Effect::CommitApply => self
                .apply_plan
                .take()
                .ok_or_else(|| "The Apply preview expired. Preview again.".to_string())
                .and_then(|plan| commit_apply(plan, true))
                .map(|mut receipt| {
                    match settings::load(&self.paths) {
                        Ok(settings) => self.settings = settings,
                        Err(error) => receipt.notes.push(format!(
                            "Apply succeeded, but the receipt could not be reloaded: {error}"
                        )),
                    }
                    self.show_receipt(receipt);
                }),
            Effect::PlanUnapply => plan_unapply(
                &self.paths,
                UnapplyOptions {
                    current_executable: self.current_executable.clone(),
                },
            )
            .map(|plan| {
                self.unapply_plan = Some(plan);
                self.screen = Screen::UnapplyPreview;
                self.selected = 0;
            }),
            Effect::CommitUnapply => self
                .unapply_plan
                .take()
                .ok_or_else(|| "The Unapply preview expired. Preview again.".to_string())
                .and_then(commit_unapply)
                .map(|receipt| {
                    self.settings.applied = None;
                    self.show_receipt(receipt);
                }),
            Effect::RunDoctor => {
                let report = doctor::run(&self.paths);
                self.status = if report.checks.is_empty() {
                    StatusState::Empty
                } else {
                    StatusState::Ready(report)
                };
                Ok(())
            }
            Effect::LoadJobs => {
                self.last_jobs_refresh = Some(Instant::now());
                match jobs::summaries(&self.paths) {
                    Ok(all_jobs) => self.refresh_jobs(all_jobs),
                    Err(error) => {
                        self.running_job_count = None;
                        self.jobs_state = if error.is_permission_denied() {
                            JobsState::PermissionDenied(error.to_string())
                        } else {
                            JobsState::Error(error.to_string())
                        };
                    }
                }
                Ok(())
            }
            Effect::LoadJobTail { job_id } => {
                self.last_tail_refresh = Some(Instant::now());
                if self.focused_job().is_some_and(|job| job.id == job_id) {
                    if self.jobs_detail.job_id.as_deref() != Some(job_id.as_str()) {
                        self.jobs_detail = JobsDetail::default();
                    }
                    self.jobs_detail.job_id = Some(job_id.clone());
                    match jobs::refresh_tail(
                        &self.paths,
                        &job_id,
                        JOB_TAIL_LINES,
                        &mut self.jobs_detail.tail,
                    ) {
                        Ok(appended) => {
                            self.jobs_detail.error = None;
                            self.jobs_detail.preserve_view_after_append(appended);
                        }
                        Err(error) => self.jobs_detail.error = Some(error),
                    }
                }
                Ok(())
            }
            Effect::RefreshJobCount => {
                self.last_jobs_refresh = Some(Instant::now());
                self.running_job_count = jobs::running_summaries(&self.paths)
                    .ok()
                    .map(|running| running.len());
                Ok(())
            }
            Effect::KillJob { job_id } => {
                jobs::kill_for_control(&self.paths, &job_id).map(|_| self.finish_job_kill())
            }
        };
        if let Err(error) = result {
            if is_doctor_effect {
                self.status = StatusState::Error(error);
                self.screen = Screen::Status;
            } else if is_kill_effect {
                self.error = Some(error);
                self.retry_effect = Some(retry_effect);
                self.screen = Screen::JobsKillFailed;
            } else {
                self.error = Some(error);
                self.retry_effect = Some(retry_effect);
                self.screen = Screen::OperationFailed;
            }
            self.selected = 0;
        }
    }

    fn handle_update(&mut self, key: KeyCode) {
        if matches!(
            key,
            KeyCode::PageUp | KeyCode::PageDown | KeyCode::Home | KeyCode::End
        ) && self.detail_viewport.handle_key(key)
        {
            return;
        }
        match key {
            KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down => {
                self.selected = 1 - self.selected.min(1);
            }
            KeyCode::Enter if self.selected == 0 => {
                if matches!(self.update_state, StartupUpdate::Available(_)) {
                    self.screen = Screen::UpdateConfirm;
                    self.selected = 0;
                } else {
                    self.start_update_check();
                }
            }
            KeyCode::Char('r') | KeyCode::Char('R') => self.start_update_check(),
            KeyCode::Enter | KeyCode::Esc => self.back_to_main(),
            _ => {}
        }
    }

    fn handle_update_confirm(&mut self, key: KeyCode) {
        match key {
            KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down => {
                self.selected = 1 - self.selected.min(1);
            }
            KeyCode::Enter if self.selected == 1 => {
                if let StartupUpdate::Available(plan) = &self.update_state {
                    self.exit_update = Some((**plan).clone());
                    self.should_quit = true;
                } else {
                    self.screen = Screen::Update;
                    self.selected = 0;
                }
            }
            KeyCode::Enter | KeyCode::Esc => {
                self.screen = Screen::Update;
                self.selected = 0;
            }
            _ => {}
        }
    }

    fn start_update_check(&mut self) {
        self.screen = Screen::UpdateChecking;
        self.pending = Some(Effect::RetryUpdate);
    }

    fn resolve_update_check(&mut self, purpose: UpdateCheckPurpose, result: StartupUpdate) {
        match purpose {
            UpdateCheckPurpose::Startup => self.resolve_startup_update(result),
            UpdateCheckPurpose::UpdatePage => self.resolve_update_page_check(result),
        }
    }

    fn resolve_startup_update(&mut self, result: StartupUpdate) {
        match result {
            available @ StartupUpdate::Available(_) => {
                self.update_state = available;
                self.toast = Some(Toast {
                    message: self.update_messages().available_title.to_string(),
                    warning: false,
                });
            }
            pending @ StartupUpdate::NpmPending { .. } => {
                self.update_state = pending;
                self.toast = Some(Toast {
                    message: self.update_messages().pending_title.to_string(),
                    warning: false,
                });
            }
            current @ StartupUpdate::NpmCurrent { .. } => {
                self.update_state = current;
            }
            StartupUpdate::Failed(error) if error.kind == CheckFailureKind::Structural => {
                self.update_state = StartupUpdate::Failed(error.clone());
                self.toast = Some(Toast {
                    message: format!("{}: {}", self.update_messages().check_failed, error.message),
                    warning: true,
                });
            }
            StartupUpdate::InstallFailed(error) => {
                self.update_state = StartupUpdate::InstallFailed(error.clone());
                self.toast = Some(Toast {
                    message: format!("{}: {error}", self.update_messages().update_failed),
                    warning: true,
                });
            }
            failed @ StartupUpdate::Failed(_) => {
                self.update_state = failed;
            }
            StartupUpdate::None => {
                self.update_state = StartupUpdate::None;
            }
        }
    }

    fn resolve_update_page_check(&mut self, result: StartupUpdate) {
        let current = matches!(
            result,
            StartupUpdate::None | StartupUpdate::NpmCurrent { .. }
        );
        self.update_state = result;
        self.screen = Screen::Update;
        self.selected = 0;
        if current {
            self.toast = Some(Toast {
                message: self.update_messages().up_to_date.to_string(),
                warning: false,
            });
        }
    }

    fn handle_language(&mut self, key: KeyCode, first_run: bool) {
        match key {
            KeyCode::Up | KeyCode::Char('k') => self.move_previous(ALL_LANGUAGES.len()),
            KeyCode::Down | KeyCode::Char('j') => self.move_next(ALL_LANGUAGES.len()),
            KeyCode::Enter => {
                self.language = ALL_LANGUAGES[self.selected];
                self.pending = Some(Effect::SaveLanguage { first_run });
            }
            KeyCode::Esc if !first_run => self.back_to_main(),
            _ => {}
        }
    }

    fn handle_main(&mut self, key: KeyCode) {
        match key {
            KeyCode::Up | KeyCode::Char('k') => self.move_previous(7),
            KeyCode::Down | KeyCode::Char('j') => self.move_next(7),
            KeyCode::Enter => match self.selected {
                0 => self.set_screen(Screen::ApplyHome),
                1 => {
                    self.config_draft = ConfigDraft::from_settings(&self.settings);
                    self.config_cursor = ConfigCursor::default();
                    self.config_viewport = ConfigViewport::default();
                    self.set_screen(Screen::Config);
                }
                2 => {
                    self.jobs_state = JobsState::Loading;
                    self.jobs_detail = JobsDetail::default();
                    self.jobs_selected = 0;
                    self.jobs_viewport = JobsViewport::default();
                    self.last_jobs_refresh = None;
                    self.last_tail_refresh = None;
                    self.screen = Screen::Jobs;
                    self.pending = Some(Effect::LoadJobs);
                }
                3 => {
                    self.set_screen(Screen::Update);
                }
                4 => {
                    self.status = StatusState::Loading;
                    self.screen = Screen::Status;
                    self.pending = Some(Effect::RunDoctor);
                }
                5 => self.set_screen(Screen::About),
                6 => {
                    self.selected = language_index(self.language);
                    self.screen = Screen::Language { first_run: false };
                }
                _ => {}
            },
            KeyCode::Char('u') | KeyCode::Char('U') => self.set_screen(Screen::Update),
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            _ => {}
        }
    }

    fn handle_apply_home(&mut self, key: KeyCode) {
        match key {
            KeyCode::Up | KeyCode::Char('k') => self.move_previous(2),
            KeyCode::Down | KeyCode::Char('j') => self.move_next(2),
            KeyCode::Enter if self.selected == 0 => {
                self.screen = Screen::ApplyLoading;
                self.pending = Some(Effect::PlanApply);
            }
            KeyCode::Enter => {
                self.screen = Screen::UnapplyLoading;
                self.pending = Some(Effect::PlanUnapply);
            }
            KeyCode::Esc => self.back_to_main(),
            _ => {}
        }
    }

    fn handle_apply_preview(&mut self, key: KeyCode) {
        if self.detail_viewport.handle_key(key) {
            return;
        }
        match key {
            KeyCode::Enter => {
                self.selected = 0;
                self.screen = if self
                    .apply_plan
                    .as_ref()
                    .and_then(ApplyPlan::token_limit_conflict)
                    .is_some()
                {
                    Screen::ApplyConflict
                } else {
                    Screen::ApplyConfirm
                };
            }
            KeyCode::Esc => {
                self.apply_plan = None;
                self.set_screen(Screen::ApplyHome);
            }
            _ => {}
        }
    }

    fn handle_binary_confirmation(&mut self, key: KeyCode, yes_screen: Screen) {
        match key {
            KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down => {
                self.selected = 1 - self.selected.min(1)
            }
            KeyCode::Enter if self.selected == 1 => {
                self.selected = 0;
                self.screen = yes_screen;
            }
            KeyCode::Enter | KeyCode::Esc => {
                self.apply_plan = None;
                self.set_screen(Screen::ApplyHome);
            }
            _ => {}
        }
    }

    fn handle_unapply_preview(&mut self, key: KeyCode) {
        if self.detail_viewport.handle_key(key) {
            return;
        }
        match key {
            KeyCode::Enter => {
                self.selected = 0;
                self.screen = Screen::UnapplyConfirm;
            }
            KeyCode::Esc => {
                self.unapply_plan = None;
                self.set_screen(Screen::ApplyHome);
            }
            _ => {}
        }
    }

    fn handle_apply_confirm(&mut self, key: KeyCode) {
        match key {
            KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down => {
                self.selected = 1 - self.selected.min(1)
            }
            KeyCode::Enter if self.selected == 1 => {
                self.screen = Screen::ApplyRunning;
                self.pending = Some(Effect::CommitApply);
            }
            KeyCode::Enter | KeyCode::Esc => {
                self.apply_plan = None;
                self.set_screen(Screen::ApplyHome);
            }
            _ => {}
        }
    }

    fn handle_unapply_confirm(&mut self, key: KeyCode) {
        match key {
            KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down => {
                self.selected = 1 - self.selected.min(1)
            }
            KeyCode::Enter if self.selected == 1 => {
                self.screen = Screen::UnapplyRunning;
                self.pending = Some(Effect::CommitUnapply);
            }
            KeyCode::Enter | KeyCode::Esc => {
                self.unapply_plan = None;
                self.set_screen(Screen::ApplyHome);
            }
            _ => {}
        }
    }

    fn handle_config(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.config_cursor = self.config_cursor.previous(),
            KeyCode::Down | KeyCode::Char('j') => self.config_cursor = self.config_cursor.next(),
            KeyCode::BackTab => self.config_cursor = self.config_cursor.previous_group(),
            KeyCode::Tab if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.config_cursor = self.config_cursor.previous_group()
            }
            KeyCode::Tab => self.config_cursor = self.config_cursor.next_group(),
            KeyCode::Left | KeyCode::Char('h') => self
                .config_draft
                .adjust(self.config_cursor.entry().item, false),
            KeyCode::Right | KeyCode::Char('l') => self
                .config_draft
                .adjust(self.config_cursor.entry().item, true),
            KeyCode::Enter => self.pending = Some(Effect::SaveConfig),
            KeyCode::Esc => self.back_to_main(),
            _ => {}
        }
    }

    fn handle_jobs(&mut self, key: KeyCode) {
        match key {
            KeyCode::Up | KeyCode::Char('k') => self.move_job_selection(false),
            KeyCode::Down | KeyCode::Char('j') => self.move_job_selection(true),
            KeyCode::Char('g') => self.select_job_edge(false),
            KeyCode::Char('G') => self.select_job_edge(true),
            KeyCode::Left | KeyCode::Char('h') => self.jobs_detail.move_horizontal(false),
            KeyCode::Right | KeyCode::Char('l') => self.jobs_detail.move_horizontal(true),
            KeyCode::PageUp => self.jobs_detail.page_output(false),
            KeyCode::PageDown => self.jobs_detail.page_output(true),
            KeyCode::Home => self.jobs_detail.jump_to_output_edge(false),
            KeyCode::End => self.jobs_detail.jump_to_output_edge(true),
            KeyCode::Char('f') | KeyCode::Char('F') => self.jobs_detail.toggle_follow(),
            KeyCode::Enter | KeyCode::Delete | KeyCode::Char('x') => {
                if let Some(job) = self
                    .focused_job()
                    .filter(|job| job.status == jobs::JobSummaryStatus::Running)
                    .cloned()
                {
                    self.pending_job = Some(job);
                    self.selected = 0;
                    self.screen = Screen::JobsKillConfirm;
                }
            }
            KeyCode::Char('r') | KeyCode::Char('R') => {
                if !matches!(self.jobs_state, JobsState::Ready(_)) {
                    self.jobs_state = JobsState::Loading;
                }
                self.last_jobs_refresh = None;
                self.pending = Some(Effect::LoadJobs);
            }
            KeyCode::Esc => self.back_to_main(),
            _ => {}
        }
    }

    fn handle_jobs_kill_confirm(&mut self, key: KeyCode) {
        match key {
            KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down => {
                self.selected = 1 - self.selected.min(1);
            }
            KeyCode::Enter if self.selected == 1 => {
                let Some(job_id) = self.pending_job.as_ref().map(|job| job.id.clone()) else {
                    self.screen = Screen::Jobs;
                    return;
                };
                self.screen = Screen::JobsKilling;
                self.pending = Some(Effect::KillJob { job_id });
            }
            KeyCode::Enter | KeyCode::Esc => {
                self.pending_job = None;
                self.selected = 0;
                self.screen = Screen::Jobs;
            }
            _ => {}
        }
    }

    fn handle_jobs_kill_failed(&mut self, key: KeyCode) {
        if self.detail_viewport.handle_key(key) {
            return;
        }
        match key {
            KeyCode::Enter => {
                let Some(effect @ Effect::KillJob { .. }) = self.retry_effect.take() else {
                    self.screen = Screen::Jobs;
                    return;
                };
                self.error = None;
                self.screen = Screen::JobsKilling;
                self.pending = Some(effect);
            }
            KeyCode::Esc => {
                self.retry_effect = None;
                self.pending_job = None;
                self.error = None;
                self.screen = Screen::Jobs;
            }
            _ => {}
        }
    }

    fn handle_status(&mut self, key: KeyCode) {
        if self.detail_viewport.handle_key(key) {
            return;
        }
        match key {
            KeyCode::Char('r') | KeyCode::Enter => {
                self.status = StatusState::Loading;
                self.pending = Some(Effect::RunDoctor);
            }
            KeyCode::Char('u') | KeyCode::Char('U') => {
                self.set_screen(Screen::Update);
            }
            KeyCode::Esc => self.back_to_main(),
            _ => {}
        }
    }

    fn handle_simple_child(&mut self, key: KeyCode) {
        if self.detail_viewport.handle_key(key) {
            return;
        }
        if key == KeyCode::Esc {
            self.back_to_main();
        }
    }

    fn handle_receipt(&mut self, key: KeyCode) {
        if self.detail_viewport.handle_key(key) {
            return;
        }
        if matches!(key, KeyCode::Enter | KeyCode::Esc) {
            self.error = None;
            self.receipt = None;
            self.back_to_main();
        }
    }

    fn handle_operation_failed(&mut self, key: KeyCode) {
        if self.detail_viewport.handle_key(key) {
            return;
        }
        match key {
            KeyCode::Enter => {
                let Some(effect) = self.retry_effect.take() else {
                    self.back_to_main();
                    return;
                };
                self.error = None;
                self.screen = match effect {
                    Effect::PlanApply => Screen::ApplyLoading,
                    Effect::PlanUnapply => Screen::UnapplyLoading,
                    Effect::CommitApply => Screen::ApplyRunning,
                    Effect::CommitUnapply => Screen::UnapplyRunning,
                    Effect::RunDoctor => {
                        self.status = StatusState::Loading;
                        Screen::Status
                    }
                    Effect::SaveConfig => Screen::Config,
                    Effect::SaveLanguage { first_run } => Screen::Language { first_run },
                    Effect::LoadJobs | Effect::LoadJobTail { .. } | Effect::RefreshJobCount => {
                        Screen::Jobs
                    }
                    Effect::KillJob { .. } => Screen::JobsKilling,
                    Effect::RetryUpdate => Screen::UpdateChecking,
                };
                self.pending = Some(effect);
            }
            KeyCode::Esc => {
                self.retry_effect = None;
                self.error = None;
                self.back_to_main();
            }
            _ => {}
        }
    }

    fn show_receipt(&mut self, receipt: OperationReceipt) {
        self.receipt = Some(receipt);
        self.screen = Screen::Receipt;
        self.selected = 0;
    }

    fn set_screen(&mut self, screen: Screen) {
        self.screen = screen;
        self.selected = 0;
        self.toast = None;
    }

    fn back_to_main(&mut self) {
        self.pending_job = None;
        self.set_screen(Screen::Main);
    }

    pub(crate) fn focused_job(&self) -> Option<&JobSummary> {
        visible_jobs(self.jobs_state.jobs())
            .get(self.jobs_selected)
            .copied()
    }

    fn refresh_jobs(&mut self, all_jobs: Vec<JobSummary>) {
        let focused_id = self.focused_job().map(|job| job.id.clone());
        let finished_id = focused_id
            .as_deref()
            .filter(|job_id| {
                all_jobs
                    .iter()
                    .any(|job| job.id == *job_id && job.status != jobs::JobSummaryStatus::Running)
            })
            .map(str::to_string)
            .or_else(|| {
                self.jobs_state.jobs().iter().find_map(|previous| {
                    all_jobs
                        .iter()
                        .any(|job| {
                            job.id == previous.id && job.status != jobs::JobSummaryStatus::Running
                        })
                        .then(|| previous.id.clone())
                })
            });
        let previous_index = self.jobs_selected;
        let running_jobs = all_jobs
            .into_iter()
            .filter(|job| job.status == jobs::JobSummaryStatus::Running)
            .collect::<Vec<_>>();
        self.running_job_count = Some(running_jobs.len());
        if let Some(job_id) = finished_id {
            self.toast = Some(Toast {
                message: self.job_messages().finished_notice.replace("{id}", &job_id),
                warning: false,
            });
        }
        if running_jobs.is_empty() {
            self.jobs_state = JobsState::Empty;
            self.jobs_selected = 0;
            self.jobs_detail = JobsDetail::default();
            self.last_tail_refresh = None;
            return;
        }
        self.jobs_state = JobsState::ready(running_jobs);
        let visible_count = visible_job_count(self.jobs_state.jobs());
        self.jobs_selected = focused_id
            .as_deref()
            .and_then(|job_id| {
                visible_jobs(self.jobs_state.jobs())
                    .iter()
                    .position(|job| job.id == job_id)
            })
            .unwrap_or_else(|| previous_index.min(visible_count - 1));
        let next_id = self
            .focused_job()
            .expect("a non-empty filtered snapshot has a focused job")
            .id
            .clone();
        if self.jobs_detail.job_id.as_deref() != Some(next_id.as_str()) {
            self.jobs_detail = JobsDetail::default();
            self.last_tail_refresh = None;
        }
    }

    fn move_job_selection(&mut self, forward: bool) {
        let len = visible_job_count(self.jobs_state.jobs());
        if len == 0 {
            return;
        }
        let previous = self.jobs_selected;
        self.jobs_selected = if forward {
            (self.jobs_selected + 1).min(len - 1)
        } else {
            self.jobs_selected.saturating_sub(1)
        };
        if self.jobs_selected != previous {
            self.jobs_detail = JobsDetail::default();
            self.last_tail_refresh = None;
        }
    }

    fn select_job_edge(&mut self, end: bool) {
        let len = visible_job_count(self.jobs_state.jobs());
        if len == 0 {
            return;
        }
        let next = if end { len - 1 } else { 0 };
        if self.jobs_selected != next {
            self.jobs_selected = next;
            self.jobs_detail = JobsDetail::default();
            self.last_tail_refresh = None;
        }
    }

    fn finish_job_kill(&mut self) {
        self.pending_job = None;
        self.retry_effect = None;
        self.error = None;
        self.jobs_state = JobsState::Loading;
        self.jobs_detail = JobsDetail::default();
        self.last_jobs_refresh = None;
        self.last_tail_refresh = None;
        self.screen = Screen::Jobs;
        self.toast = Some(Toast {
            message: self.job_messages().kill_success.to_string(),
            warning: false,
        });
    }

    fn move_previous(&mut self, len: usize) {
        self.selected = if self.selected == 0 {
            len.saturating_sub(1)
        } else {
            self.selected - 1
        };
    }

    fn move_next(&mut self, len: usize) {
        self.selected = (self.selected + 1) % len.max(1);
    }
}

fn language_index(language: Language) -> usize {
    ALL_LANGUAGES
        .iter()
        .position(|candidate| *candidate == language)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{App, Effect, Screen};
    use crate::control::paths::ControlPaths;
    use crate::control::settings::{Tier, ToolBudgetLevel, UpdateSource};
    use crate::shell::jobs::{JobSourceSummary, JobSummary, JobSummaryStatus};
    use crate::tui::config::{ConfigCursor, ConfigItemId, ConfigValue};
    use crate::tui::jobs::{JobsDetail, JobsState};
    use crate::update::{
        CheckFailure, CheckFailureKind, NpmDiscovery, NpmRegistryProbe, NpmVersionAuthority,
        StartupUpdate, UpdatePlan,
    };
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn fixture() -> (tempfile::TempDir, App) {
        let temp = tempfile::tempdir().unwrap();
        let paths = ControlPaths::for_home(temp.path());
        std::fs::create_dir_all(&paths.codex_dir).unwrap();
        let executable = temp.path().join(if cfg!(windows) {
            "source.exe"
        } else {
            "source"
        });
        std::fs::write(&executable, b"binary").unwrap();
        let app = App::for_test(paths, executable);
        (temp, app)
    }

    fn pending_discovery(target_version: &str) -> NpmDiscovery {
        NpmDiscovery {
            source_policy: "auto".to_string(),
            configured_registry: Some("https://registry.npmmirror.com/".to_string()),
            target_version: target_version.to_string(),
            authority: NpmVersionAuthority::Official,
            github_version: Some(target_version.to_string()),
            official_version: Some(target_version.to_string()),
            platform_package: "@fastctx/test-platform".to_string(),
            probes: vec![NpmRegistryProbe {
                source_name: "npmmirror".to_string(),
                registry: "https://registry.npmmirror.com/".to_string(),
                reachable: true,
                latest_version: Some("0.1.0".to_string()),
                main_package_ready: false,
                platform_package_ready: false,
                error: None,
                error_kind: None,
            }],
            selected_registry: None,
            selected_source: None,
            selection_reason: "the configured source is still propagating".to_string(),
        }
    }

    fn job(id: &str) -> JobSummary {
        job_from(id, "source-1", JobSummaryStatus::Running)
    }

    fn job_from(id: &str, source_key: &str, status: JobSummaryStatus) -> JobSummary {
        JobSummary {
            id: id.to_string(),
            command: format!("printf {id}"),
            cwd: "/workspace".to_string(),
            started_at: "2026-07-16T10:00:00Z".to_string(),
            status,
            source: JobSourceSummary {
                key: source_key.to_string(),
                tag: source_key.to_string(),
                server_pid: 7,
                parent_executable: Some("codex".to_string()),
                server_cwd: format!("/{source_key}"),
            },
        }
    }

    #[test]
    fn first_run_requires_language_selection_before_main_menu() {
        let (_temp, mut app) = fixture();
        assert!(matches!(app.screen, Screen::Language { first_run: true }));
        app.handle_key(key(KeyCode::Enter));
        assert!(app.has_pending_effect());
        app.execute_pending();
        assert_eq!(app.screen, Screen::Main);
        assert!(app.settings.language.is_some());
    }

    #[test]
    fn apply_flow_reaches_preview_and_receipt_from_one_frozen_plan() {
        let (_temp, mut app) = fixture();
        app.settings.language = Some("en".to_string());
        app.screen = Screen::Main;
        app.selected = 0;
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.screen, Screen::ApplyHome);
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.screen, Screen::ApplyLoading);
        app.execute_pending();
        assert_eq!(app.screen, Screen::ApplyPreview);
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.screen, Screen::ApplyConfirm);
        app.handle_key(key(KeyCode::Right));
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.screen, Screen::ApplyRunning);
        app.execute_pending();
        assert_eq!(app.screen, Screen::Receipt);
        assert!(app.receipt.as_ref().unwrap().changed_targets >= 3);
    }

    #[test]
    fn unapply_goes_straight_from_apply_home_to_preview() {
        let (_temp, mut app) = fixture();
        app.screen = Screen::ApplyHome;
        app.selected = 1;
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.screen, Screen::UnapplyLoading);
        app.execute_pending();
        assert_eq!(app.screen, Screen::UnapplyPreview);
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.screen, Screen::UnapplyConfirm);
    }

    #[test]
    fn tui_unapply_cancel_is_zero_write_then_confirm_restores_user_bytes() {
        let (temp, mut app) = fixture();
        let config = concat!(
            "# user config\n",
            "tool_output_token_limit = 10000 # exact\n",
            "\n",
            "[mcp_servers.other]\n",
            "command = 'other'\n",
        );
        let agents = "# User rules\n\nKeep this exact.\n";
        std::fs::write(&app.paths.codex_config, config).unwrap();
        std::fs::write(&app.paths.codex_agents, agents).unwrap();

        app.screen = Screen::ApplyHome;
        app.selected = 0;
        app.handle_key(key(KeyCode::Enter));
        app.execute_pending();
        app.handle_key(key(KeyCode::Enter));
        app.handle_key(key(KeyCode::Right));
        app.handle_key(key(KeyCode::Enter));
        app.execute_pending();
        assert_eq!(app.screen, Screen::Receipt);
        app.handle_key(key(KeyCode::Enter));

        app.screen = Screen::ApplyHome;
        app.selected = 1;
        app.handle_key(key(KeyCode::Enter));
        app.execute_pending();
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.screen, Screen::UnapplyConfirm);
        let before_cancel = file_tree(temp.path());
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.screen, Screen::ApplyHome);
        assert_eq!(file_tree(temp.path()), before_cancel);

        app.selected = 1;
        app.handle_key(key(KeyCode::Enter));
        app.execute_pending();
        app.handle_key(key(KeyCode::Enter));
        app.handle_key(key(KeyCode::Right));
        app.handle_key(key(KeyCode::Enter));
        app.execute_pending();

        assert_eq!(app.screen, Screen::Receipt);
        assert_eq!(
            std::fs::read(&app.paths.codex_config).unwrap(),
            config.as_bytes()
        );
        assert_eq!(
            std::fs::read(&app.paths.codex_agents).unwrap(),
            agents.as_bytes()
        );
    }

    #[test]
    fn shared_limit_confirmation_defaults_to_no_and_cancel_writes_nothing() {
        let (_temp, mut app) = fixture();
        std::fs::write(&app.paths.codex_config, b"tool_output_token_limit = 9000\n").unwrap();
        app.screen = Screen::ApplyHome;
        app.selected = 0;
        app.handle_key(key(KeyCode::Enter));
        app.execute_pending();
        assert_eq!(app.screen, Screen::ApplyPreview);
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.screen, Screen::ApplyConflict);
        assert_eq!(app.selected, 0);
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.screen, Screen::ApplyHome);
        assert_eq!(
            std::fs::read(&app.paths.codex_config).unwrap(),
            b"tool_output_token_limit = 9000\n"
        );
        assert!(!app.paths.fastctx_config.exists());
    }

    #[test]
    fn operation_failure_has_a_retry_path_back_to_a_fresh_preview() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ControlPaths::for_home(temp.path());
        let executable = temp.path().join("source");
        std::fs::write(&executable, b"binary").unwrap();
        std::fs::write(&paths.codex_dir, b"blocks profile directory creation").unwrap();
        let mut app = App::for_test(paths.clone(), executable);
        app.screen = Screen::ApplyHome;
        app.selected = 0;
        app.handle_key(key(KeyCode::Enter));
        app.execute_pending();
        assert_eq!(app.screen, Screen::OperationFailed);
        assert!(
            app.error
                .as_deref()
                .is_some_and(|error| error.contains("is not a directory"))
        );

        std::fs::remove_file(&paths.codex_dir).unwrap();
        std::fs::create_dir_all(&paths.codex_dir).unwrap();
        std::fs::write(&paths.codex_config, b"tool_output_token_limit = 7000\n").unwrap();
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.screen, Screen::ApplyLoading);
        app.execute_pending();
        assert_eq!(app.screen, Screen::ApplyPreview);
        assert_eq!(
            app.apply_plan
                .as_ref()
                .and_then(|plan| plan.token_limit_conflict())
                .map(|conflict| conflict.current),
            Some(7_000)
        );
    }

    #[test]
    fn config_navigation_walks_parent_then_children_and_wraps() {
        let (_temp, mut app) = fixture();
        app.settings.language = Some("en".to_string());
        app.screen = Screen::Config;
        app.config_cursor = ConfigCursor::default();

        for expected in [
            ConfigItemId::OutputTier,
            ConfigItemId::ReadBudget,
            ConfigItemId::GrepBudget,
            ConfigItemId::GlobBudget,
            ConfigItemId::RunBudget,
            ConfigItemId::JobOutputBudget,
            ConfigItemId::FastShell,
            ConfigItemId::JobStorageLimit,
            ConfigItemId::MaxRunningJobs,
            ConfigItemId::JobListLimit,
            ConfigItemId::UpdateAutoCheck,
            ConfigItemId::UpdateSource,
        ] {
            assert_eq!(app.config_cursor.entry().item, expected);
            app.handle_key(key(KeyCode::Down));
        }
        assert_eq!(app.config_cursor, ConfigCursor::default());
        app.handle_key(key(KeyCode::Up));
        assert_eq!(app.config_cursor.entry().item, ConfigItemId::UpdateSource);
    }

    #[test]
    fn config_tab_and_shift_tab_jump_between_group_parents() {
        let (_temp, mut app) = fixture();
        app.settings.language = Some("en".to_string());
        app.screen = Screen::Config;
        app.config_cursor = ConfigCursor::default().next().next();

        app.handle_key(key(KeyCode::Tab));
        assert_eq!(app.config_cursor.entry().item, ConfigItemId::FastShell);
        app.handle_key(key(KeyCode::Tab));
        assert_eq!(
            app.config_cursor.entry().item,
            ConfigItemId::UpdateAutoCheck
        );
        app.handle_key(key(KeyCode::Tab));
        assert_eq!(app.config_cursor.entry().item, ConfigItemId::OutputTier);
        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT));
        assert_eq!(
            app.config_cursor.entry().item,
            ConfigItemId::UpdateAutoCheck
        );
        app.handle_key(key(KeyCode::BackTab));
        assert_eq!(app.config_cursor.entry().item, ConfigItemId::FastShell);
    }

    #[test]
    fn fastshell_toggle_saves_on_enter_and_takes_effect_only_on_apply() {
        let (_temp, mut app) = fixture();
        app.settings.language = Some("en".to_string());
        app.screen = Screen::Config;
        for _ in 0..6 {
            app.handle_key(key(KeyCode::Down));
        }
        assert_eq!(app.config_cursor.entry().item, ConfigItemId::FastShell);
        app.handle_key(key(KeyCode::Right));
        assert!(!app.settings.fastshell.enabled);

        app.handle_key(key(KeyCode::Enter));
        app.execute_pending();

        assert!(app.settings.fastshell.enabled);
        assert!(!app.settings.fastedit.enabled);
        assert!(app.settings.applied.is_none());
        let persisted = crate::control::settings::load(&app.paths).unwrap();
        assert!(persisted.fastshell.enabled);
        assert!(!persisted.fastedit.enabled);
    }

    #[test]
    fn current_user_job_limits_save_immediately_without_apply() {
        let (_temp, mut app) = fixture();
        app.settings.language = Some("en".to_string());
        app.screen = Screen::Config;
        for _ in 0..7 {
            app.handle_key(key(KeyCode::Down));
        }
        assert_eq!(
            app.config_cursor.entry().item,
            ConfigItemId::JobStorageLimit
        );
        app.handle_key(key(KeyCode::Right));
        app.handle_key(key(KeyCode::Down));
        assert_eq!(app.config_cursor.entry().item, ConfigItemId::MaxRunningJobs);
        app.handle_key(key(KeyCode::Right));
        app.handle_key(key(KeyCode::Down));
        assert_eq!(app.config_cursor.entry().item, ConfigItemId::JobListLimit);
        app.handle_key(key(KeyCode::Right));
        app.handle_key(key(KeyCode::Enter));
        app.execute_pending();

        assert_eq!(app.screen, Screen::Main);
        assert_eq!(app.settings.fastshell.job_storage_limit_mib, 2_048);
        assert_eq!(app.settings.fastshell.max_running_jobs, 256);
        assert_eq!(app.settings.fastshell.job_list_limit, 50);
        assert!(app.settings.applied.is_none());
        assert!(
            app.toast.as_ref().is_some_and(|toast| {
                toast.message.contains(app.job_messages().user_limit_note)
            })
        );
        let persisted = crate::control::settings::load(&app.paths).unwrap();
        assert_eq!(persisted.fastshell.job_storage_limit_mib, 2_048);
        assert_eq!(persisted.fastshell.max_running_jobs, 256);
        assert_eq!(persisted.fastshell.job_list_limit, 50);
        assert!(persisted.applied.is_none());
    }

    #[test]
    fn update_preferences_save_immediately_without_apply() {
        let (_temp, mut app) = fixture();
        app.settings.language = Some("en".to_string());
        app.screen = Screen::Config;
        app.config_cursor = ConfigCursor::default().next_group().next_group();
        assert_eq!(
            app.config_cursor.entry().item,
            ConfigItemId::UpdateAutoCheck
        );

        app.handle_key(key(KeyCode::Right));
        app.handle_key(key(KeyCode::Down));
        assert_eq!(app.config_cursor.entry().item, ConfigItemId::UpdateSource);
        app.handle_key(key(KeyCode::Right));
        app.handle_key(key(KeyCode::Enter));
        app.execute_pending();

        assert_eq!(app.screen, Screen::Main);
        assert!(!app.settings.update.auto_check);
        assert_eq!(app.settings.update.source, UpdateSource::NpmConfig);
        assert!(app.settings.applied.is_none());
        let persisted = crate::control::settings::load(&app.paths).unwrap();
        assert!(!persisted.update.auto_check);
        assert_eq!(persisted.update.source, UpdateSource::NpmConfig);
        assert!(persisted.applied.is_none());
    }

    #[test]
    fn jobs_refresh_preserves_the_focused_id_and_clamps_when_it_disappears() {
        let (_temp, mut app) = fixture();
        app.screen = Screen::Jobs;
        app.jobs_state = JobsState::ready(vec![job("j-000001"), job("j-000002")]);
        app.jobs_selected = 1;
        app.jobs_detail.job_id = Some("j-000002".to_string());

        app.refresh_jobs(vec![job("j-000003"), job("j-000002"), job("j-000004")]);
        assert_eq!(app.jobs_selected, 1);
        assert_eq!(app.focused_job().unwrap().id, "j-000002");
        assert_eq!(app.jobs_detail.job_id.as_deref(), Some("j-000002"));

        app.refresh_jobs(vec![job("j-000003"), job("j-000004")]);
        assert_eq!(app.jobs_selected, 1);
        assert_eq!(app.focused_job().unwrap().id, "j-000004");
        assert!(app.jobs_detail.job_id.is_none());

        app.refresh_jobs(Vec::new());
        assert!(matches!(app.jobs_state, JobsState::Empty));
        assert_eq!(app.jobs_selected, 0);
        assert!(app.jobs_detail.job_id.is_none());
    }

    #[test]
    fn jobs_empty_enter_is_inert_and_escape_returns_to_main() {
        let (_temp, mut app) = fixture();
        app.screen = Screen::Jobs;
        app.jobs_state = JobsState::Empty;

        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.screen, Screen::Jobs);
        assert!(app.pending_job.is_none());
        assert!(!app.has_pending_effect());

        app.handle_key(key(KeyCode::Esc));
        assert_eq!(app.screen, Screen::Main);
    }

    #[test]
    fn jobs_dashboard_aggregates_running_jobs_only_and_exposes_navigation_keys() {
        let (_temp, mut app) = fixture();
        app.screen = Screen::Jobs;
        app.refresh_jobs(vec![
            job_from("j-a-run", "source-a", JobSummaryStatus::Running),
            job_from("j-b-run", "source-b", JobSummaryStatus::Running),
            job_from("j-a-done", "source-a", JobSummaryStatus::Exited(0)),
        ]);
        assert_eq!(app.running_job_count, Some(2));
        assert_eq!(app.jobs_state.jobs().len(), 2);
        assert!(
            app.jobs_state
                .jobs()
                .iter()
                .all(|job| job.status == JobSummaryStatus::Running)
        );
        assert_eq!(app.focused_job().unwrap().id, "j-a-run");

        app.jobs_detail.job_id = Some("j-a-run".to_string());
        app.jobs_detail.tail.lines = vec!["0123456789abcdef".to_string()];
        app.handle_key(key(KeyCode::Right));
        assert_eq!(app.jobs_detail.horizontal_offset, 8);
        app.handle_key(key(KeyCode::PageUp));
        assert!(!app.jobs_detail.follow_tail);

        app.handle_key(key(KeyCode::Tab));
        assert_eq!(app.focused_job().unwrap().id, "j-a-run");

        app.handle_key(key(KeyCode::Char('R')));
        assert!(matches!(app.pending, Some(Effect::LoadJobs)));
    }

    #[test]
    fn a_finished_running_job_disappears_with_an_agent_output_notice() {
        let (_temp, mut app) = fixture();
        app.screen = Screen::Jobs;
        app.jobs_state = JobsState::ready(vec![job("j-000001"), job("j-000002")]);
        app.jobs_selected = 0;

        app.refresh_jobs(vec![
            job_from("j-000001", "source-1", JobSummaryStatus::Exited(0)),
            job("j-000002"),
        ]);

        assert_eq!(app.jobs_state.jobs().len(), 1);
        assert_eq!(app.focused_job().unwrap().id, "j-000002");
        let toast = app.toast.as_ref().expect("completion notice");
        assert!(toast.message.contains("j-000001"));
        assert!(toast.message.contains("job_output"));
        assert!(!toast.warning);
    }

    #[test]
    fn job_kill_confirmation_defaults_to_no_and_failure_can_retry_or_escape() {
        let (_temp, mut app) = fixture();
        app.screen = Screen::Jobs;
        app.jobs_state = JobsState::ready(vec![job("j-000001")]);

        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.screen, Screen::JobsKillConfirm);
        assert_eq!(app.selected, 0);
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.screen, Screen::Jobs);
        assert!(app.pending_job.is_none());
        assert!(!app.has_pending_effect());

        app.handle_key(key(KeyCode::Enter));
        app.handle_key(key(KeyCode::Esc));
        assert_eq!(app.screen, Screen::Jobs);
        assert!(app.pending_job.is_none());

        app.handle_key(key(KeyCode::Enter));
        app.handle_key(key(KeyCode::Right));
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.screen, Screen::JobsKilling);
        assert!(matches!(
            app.pending,
            Some(Effect::KillJob { ref job_id }) if job_id == "j-000001"
        ));
        app.execute_pending();
        assert_eq!(app.screen, Screen::JobsKillFailed);
        assert!(
            app.error
                .as_deref()
                .is_some_and(|error| error.contains("No such job"))
        );
        assert!(matches!(
            app.retry_effect,
            Some(Effect::KillJob { ref job_id }) if job_id == "j-000001"
        ));

        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.screen, Screen::JobsKilling);
        assert!(app.has_pending_effect());
        app.execute_pending();
        assert_eq!(app.screen, Screen::JobsKillFailed);
        app.handle_key(key(KeyCode::Esc));
        assert_eq!(app.screen, Screen::Jobs);
        assert!(app.error.is_none());
        assert!(app.pending_job.is_none());
        assert!(app.retry_effect.is_none());
    }

    #[test]
    fn successful_job_kill_returns_to_loading_with_a_refreshable_toast() {
        let (_temp, mut app) = fixture();
        app.screen = Screen::JobsKilling;
        app.jobs_state = JobsState::ready(vec![job("j-000001")]);
        app.jobs_detail = JobsDetail {
            job_id: Some("j-000001".to_string()),
            ..Default::default()
        };
        app.pending_job = Some(job("j-000001"));
        app.error = Some("old failure".to_string());
        app.finish_job_kill();

        assert_eq!(app.screen, Screen::Jobs);
        assert!(matches!(app.jobs_state, JobsState::Loading));
        assert!(app.jobs_detail.job_id.is_none());
        assert!(app.pending_job.is_none());
        assert!(app.error.is_none());
        assert!(app.last_jobs_refresh.is_none());
        assert!(app.last_tail_refresh.is_none());
        assert_eq!(
            app.toast,
            Some(super::Toast {
                message: app.job_messages().kill_success.to_string(),
                warning: false,
            })
        );
    }

    #[test]
    fn startup_update_receiver_never_blocks_the_home_screen_and_resolves_in_place() {
        let (_temp, mut app) = fixture();
        app.settings.language = Some("en".to_string());
        app.screen = Screen::Main;
        let (sender, receiver) = std::sync::mpsc::channel();
        app.set_startup_update_check(receiver);

        app.poll_update_check();
        assert_eq!(app.screen, Screen::Main);
        assert_eq!(app.update_state, StartupUpdate::None);
        assert!(app.toast.is_none());

        let plan = UpdatePlan::GithubRelease {
            target_version: "0.2.0".to_string(),
            archive_name: "fixture.zip".to_string(),
            archive_url: "https://github.com/yc-duan/fastctx/releases/download/v0.2.0/fixture.zip"
                .to_string(),
            checksums_url: "https://github.com/yc-duan/fastctx/releases/download/v0.2.0/SHA256SUMS"
                .to_string(),
        };
        sender
            .send(StartupUpdate::Available(Box::new(plan.clone())))
            .unwrap();
        app.poll_update_check();
        assert_eq!(app.screen, Screen::Main);
        assert_eq!(app.update_state, StartupUpdate::Available(Box::new(plan)));
        assert_eq!(app.toast.as_ref().map(|toast| toast.warning), Some(false));

        app.handle_key(key(KeyCode::Char('u')));
        assert_eq!(app.screen, Screen::Update);
    }

    #[test]
    fn startup_update_failures_follow_the_quiet_transient_single_warning_structural_contract() {
        let (_temp, mut app) = fixture();
        app.settings.language = Some("en".to_string());
        app.screen = Screen::Main;
        let (sender, receiver) = std::sync::mpsc::channel();
        app.set_startup_update_check(receiver);
        sender
            .send(StartupUpdate::Failed(CheckFailure {
                kind: CheckFailureKind::Transient,
                message: "HTTP 429".to_string(),
            }))
            .unwrap();
        app.poll_update_check();
        assert!(app.toast.is_none());

        let (sender, receiver) = std::sync::mpsc::channel();
        app.set_startup_update_check(receiver);
        sender
            .send(StartupUpdate::Failed(CheckFailure {
                kind: CheckFailureKind::Structural,
                message: "latest tag is invalid".to_string(),
            }))
            .unwrap();
        app.poll_update_check();
        let toast = app.toast.take().unwrap();
        assert!(toast.warning);
        assert!(toast.message.contains("latest tag is invalid"));
        app.poll_update_check();
        assert!(app.toast.is_none(), "the structural warning repeated");
    }

    #[test]
    fn startup_update_never_preempts_first_run_and_the_update_page_confirms_installation() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ControlPaths::for_home(temp.path());
        let plan = UpdatePlan::GithubRelease {
            target_version: "0.2.0".to_string(),
            archive_name: "fixture.zip".to_string(),
            archive_url: "https://github.com/yc-duan/fastctx/releases/download/v0.2.0/fixture.zip"
                .to_string(),
            checksums_url: "https://github.com/yc-duan/fastctx/releases/download/v0.2.0/SHA256SUMS"
                .to_string(),
        };
        let mut app = App::load_with_startup(
            paths.clone(),
            StartupUpdate::Available(Box::new(plan.clone())),
            None,
        )
        .unwrap();
        assert_eq!(app.screen, Screen::Language { first_run: true });
        assert_eq!(
            app.toast.as_ref().map(|toast| toast.message.as_str()),
            Some(app.update_messages().available_title)
        );
        app.set_screen(Screen::Update);
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.screen, Screen::UpdateConfirm);
        assert!(!app.should_quit);
        app.handle_key(key(KeyCode::Right));
        app.handle_key(key(KeyCode::Enter));
        assert!(app.should_quit);
        assert_eq!(app.take_update_plan(), Some(plan));

        let discovery = pending_discovery("0.2.0");
        let mut app = App::load_with_startup(
            paths,
            StartupUpdate::NpmPending {
                target_version: "0.2.0".to_string(),
                discovery: Box::new(discovery),
            },
            None,
        )
        .unwrap();
        assert_eq!(app.screen, Screen::Language { first_run: true });
        assert_eq!(
            app.toast.as_ref().map(|toast| toast.message.as_str()),
            Some(app.update_messages().pending_title)
        );
        assert!(!app.should_quit);
        assert!(app.take_update_plan().is_none());

        let temp = tempfile::tempdir().unwrap();
        let mut app = App::load_with_startup(
            ControlPaths::for_home(temp.path()),
            StartupUpdate::InstallFailed("injected failure".to_string()),
            None,
        )
        .unwrap();
        assert_eq!(app.screen, Screen::Language { first_run: true });
        let toast = app.toast.take().unwrap();
        assert!(toast.warning);
        assert!(toast.message.contains(app.update_messages().update_failed));
        assert!(toast.message.contains("injected failure"));
    }

    #[test]
    fn config_nested_adjustments_save_on_enter_and_discard_on_escape() {
        let (_temp, mut app) = fixture();
        app.settings.language = Some("en".to_string());
        app.screen = Screen::Config;
        app.config_cursor = ConfigCursor::default();
        app.handle_key(key(KeyCode::Right));
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Right));
        assert_eq!(
            app.config_draft.value(ConfigItemId::OutputTier),
            ConfigValue::Tier(Tier::High)
        );
        assert_eq!(
            app.config_draft.value(ConfigItemId::ReadBudget),
            ConfigValue::Budget(ToolBudgetLevel::Percent75)
        );
        assert_eq!(app.settings.tier, Tier::Standard);
        assert_eq!(app.settings.tool_budgets.read, ToolBudgetLevel::Inherit);
        app.handle_key(key(KeyCode::Esc));
        assert_eq!(app.screen, Screen::Main);
        assert_eq!(app.settings.tier, Tier::Standard);
        assert!(!app.paths.fastctx_config.exists());

        app.selected = 1;
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.screen, Screen::Config);
        app.handle_key(key(KeyCode::Right));
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Right));
        for _ in 0..3 {
            app.handle_key(key(KeyCode::Down));
        }
        app.handle_key(key(KeyCode::Right));
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Right));
        app.handle_key(key(KeyCode::Enter));
        app.execute_pending();
        assert_eq!(app.screen, Screen::Main);
        assert_eq!(app.settings.tier, Tier::High);
        assert_eq!(app.settings.tool_budgets.read, ToolBudgetLevel::Percent75);
        assert_eq!(app.settings.tool_budgets.run, ToolBudgetLevel::Percent75);
        assert_eq!(
            app.settings.tool_budgets.job_output,
            ToolBudgetLevel::Percent75
        );
        let persisted = crate::control::settings::load(&app.paths).unwrap();
        assert_eq!(persisted.tier, Tier::High);
        assert_eq!(persisted.tool_budgets.read, ToolBudgetLevel::Percent75);
        assert_eq!(persisted.tool_budgets.run, ToolBudgetLevel::Percent75);
        assert_eq!(
            persisted.tool_budgets.job_output,
            ToolBudgetLevel::Percent75
        );
    }

    #[test]
    fn config_save_failure_keeps_the_draft_and_retries_cleanly() {
        let (_temp, mut app) = fixture();
        app.settings.language = Some("en".to_string());
        app.screen = Screen::Config;
        app.config_cursor = ConfigCursor::default();
        app.handle_key(key(KeyCode::Right));
        std::fs::write(&app.paths.fastctx_dir, b"blocks directory creation").unwrap();

        app.handle_key(key(KeyCode::Enter));
        app.execute_pending();
        assert_eq!(app.screen, Screen::OperationFailed);
        assert!(app.error.is_some());
        assert_eq!(
            app.config_draft.value(ConfigItemId::OutputTier),
            ConfigValue::Tier(Tier::High)
        );
        assert_eq!(app.settings.tier, Tier::Standard);

        std::fs::remove_file(&app.paths.fastctx_dir).unwrap();
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.screen, Screen::Config);
        app.execute_pending();
        assert_eq!(app.screen, Screen::Main);
        assert_eq!(app.settings.tier, Tier::High);
    }

    fn file_tree(root: &std::path::Path) -> Vec<(std::path::PathBuf, Vec<u8>)> {
        let mut files = walkdir::WalkDir::new(root)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file())
            .map(|entry| {
                (
                    entry.path().strip_prefix(root).unwrap().to_path_buf(),
                    std::fs::read(entry.path()).unwrap(),
                )
            })
            .collect::<Vec<_>>();
        files.sort_by(|left, right| left.0.cmp(&right.0));
        files
    }
}
