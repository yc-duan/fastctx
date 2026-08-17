//! Pure TUI state transitions and controlled I/O effects.

use super::budget_editor::{self, BudgetEditor};
use super::config::{self, ConfigCursor, ConfigDraft, ConfigItemId, ConfigValue, ConfigViewport};
use super::jobs::{JobsDetail, JobsState, JobsViewport, visible_job_count, visible_jobs};
use super::migration::{self as migration_copy, MigrationMessages};
use super::update::{self as update_copy, UpdateMessages};
use crate::control::apply::{
    ApplyOptions, ApplyPlan, OperationReceipt, UnapplyOptions, UnapplyPlan, commit_apply,
    commit_unapply, plan_apply, plan_unapply, plan_unapply_all,
};
use crate::control::config_i18n::{self, ConfigMessages};
use crate::control::doctor::{self, DoctorReport};
use crate::control::guard_i18n::{self, GuardMessages};
use crate::control::i18n::{ALL_LANGUAGES, Language, Messages};
use crate::control::job_i18n::{self, JobMessages};
use crate::control::link::{self, LinkState};
use crate::control::paths::ControlPaths;
use crate::control::provider::{self, EffectiveOutput, GuardReason};
use crate::control::settings::{self, FastCtxSettings};
use crate::search_parallelism::{self, SearchParallelismInputError};
use crate::shell::jobs::{self, JobSummary};
use crate::update::{CheckFailure, CheckFailureKind, StartupUpdate, UpdatePlan};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

const JOB_LIST_REFRESH: Duration = Duration::from_secs(1);
const JOB_TAIL_REFRESH: Duration = Duration::from_millis(300);
const JOB_TAIL_LINES: usize = 512;
const STARTUP_UPDATE_GATE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Screen {
    MigrationNotice,
    Update,
    UpdateChecking,
    UpdateConfirm,
    Language { first_run: bool },
    Main,
    Connections,
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
    ConfigCpuEdit,
    ConfigBudgetEdit(ConfigItemId),
    ConfigOutputGuardConfirm,
    ConfigDiscardConfirm,
    ConfigResetConfirm,
    ConfigResetting,
    ConfigSaving,
    Jobs,
    JobsKillConfirm,
    JobsKilling,
    JobsKillFailed,
    Status,
    About,
    Receipt,
    OperationFailed,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum ActiveHost {
    #[default]
    Codex,
    DeepSeekHarness,
    All,
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

fn finalize_notice_toast(
    messages: &UpdateMessages,
    notice: crate::update::FinalizeNotice,
) -> Toast {
    let (base, mut warning) = match notice.outcome {
        crate::update::FinalizeOutcome::Updated => (
            messages.updated.replace("{version}", &notice.version),
            false,
        ),
        crate::update::FinalizeOutcome::RuntimeUpdated => (
            messages
                .updated_runtime
                .replace("{version}", &notice.version),
            false,
        ),
        crate::update::FinalizeOutcome::RuntimeUnchanged(detail) => (
            format!(
                "{}: {detail}",
                messages
                    .runtime_unchanged
                    .replace("{version}", &notice.version)
            ),
            true,
        ),
    };
    let mut lines = vec![base];
    match notice.guidance {
        crate::update::FinalizeGuidanceOutcome::NotApplied
        | crate::update::FinalizeGuidanceOutcome::Current => {}
        crate::update::FinalizeGuidanceOutcome::Refreshed => {
            lines.push(messages.guidance_apply_required.to_string());
            lines.push(messages.guidance_refreshed.to_string());
            warning = true;
        }
        crate::update::FinalizeGuidanceOutcome::ApplyRequired => {
            lines.push(messages.guidance_apply_required.to_string());
            warning = true;
        }
        crate::update::FinalizeGuidanceOutcome::Unchanged(detail) => {
            lines.push(messages.guidance_apply_required.to_string());
            lines.push(messages.guidance_unchanged.replace("{detail}", &detail));
            warning = true;
        }
    }
    // Apply must precede the restart when guidance still needs an explicit ownership receipt.
    lines.push(messages.restart_codex.to_string());
    Toast {
        message: lines.join("\n"),
        warning,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Effect {
    RetryUpdate,
    SaveLanguage { first_run: bool },
    SaveConfig,
    ResetConfig,
    PlanApply,
    CommitApply,
    PlanUnapply,
    CommitUnapply,
    PlanDshApply,
    CommitDshApply,
    PlanDshUnapply,
    CommitDshUnapply,
    PlanUnapplyAll,
    RunDoctor,
    RunDshDoctor,
    LoadJobs,
    LoadJobTail { job_id: String },
    RefreshJobCount,
    KillJob { job_id: String },
}

/// Editable search CPU limit plus the last validation failure.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct CpuLimitEditor {
    pub(crate) input: String,
    pub(crate) error: Option<SearchParallelismInputError>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum UpdateCheckPurpose {
    Startup,
    UpdatePage,
}

struct ActiveUpdateCheck {
    purpose: UpdateCheckPurpose,
    receiver: Receiver<StartupUpdate>,
    startup_deadline: Option<Instant>,
}

pub(crate) struct App {
    pub paths: ControlPaths,
    pub settings: FastCtxSettings,
    pub(crate) provider_detection: provider::ProviderDetection,
    pub language: Language,
    pub screen: Screen,
    pub selected: usize,
    pub config_draft: ConfigDraft,
    pub config_cursor: ConfigCursor,
    pub config_viewport: ConfigViewport,
    pub(crate) cpu_limit_editor: CpuLimitEditor,
    pub(crate) budget_editor: BudgetEditor,
    pub jobs_state: JobsState,
    pub jobs_detail: JobsDetail,
    pub jobs_selected: usize,
    pub jobs_viewport: JobsViewport,
    pub(crate) detail_viewport: DetailViewport,
    pub pending_job: Option<JobSummary>,
    pub running_job_count: Option<usize>,
    pub(crate) link_state: LinkState,
    pub(crate) active_host: ActiveHost,
    pub(crate) dsh_status: Result<(String, String), String>,
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
    pub(crate) dsh_plan: Option<crate::control::dsh::Plan>,
    pending: Option<Effect>,
    retry_effect: Option<Effect>,
    last_jobs_refresh: Option<Instant>,
    last_tail_refresh: Option<Instant>,
    migration_notice_pending: bool,
    startup_update_check_requested: bool,
    update_check: Option<ActiveUpdateCheck>,
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
        let startup_settings = settings::load_for_startup(&paths)?;
        let migration_notice_pending = startup_settings.migration_notice;
        let settings = startup_settings.settings;
        let paths = settings::paths_for_integrations(&paths, &settings)?;
        let provider_detection = provider::detect_path(&paths.codex_config);
        let running_job_count = jobs::running_summaries(&paths)
            .ok()
            .map(|running| running.len());
        let link_state = link::link_state(&paths, settings.integrations.codex.as_ref());
        let language = settings
            .language
            .as_deref()
            .and_then(Language::parse)
            .unwrap_or_else(Language::detect);
        let home_screen = if settings.language.is_none() {
            Screen::Language { first_run: true }
        } else if migration_notice_pending {
            Screen::MigrationNotice
        } else {
            Screen::Main
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
        let startup_notice = startup_notice
            .map(|notice| finalize_notice_toast(update_copy::messages(notice_language), notice));
        let startup_failure = match &startup_update {
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
            StartupUpdate::None
            | StartupUpdate::NpmCurrent { .. }
            | StartupUpdate::Available(_)
            | StartupUpdate::NpmPending { .. }
            | StartupUpdate::Failed(_) => None,
        };
        let dsh_status = crate::control::dsh::status(&paths);
        Ok(Self {
            config_draft: ConfigDraft::from_settings(&settings),
            config_cursor: ConfigCursor::default(),
            config_viewport: ConfigViewport::default(),
            cpu_limit_editor: CpuLimitEditor::default(),
            budget_editor: BudgetEditor::default(),
            jobs_state: JobsState::Loading,
            jobs_detail: JobsDetail::default(),
            jobs_selected: 0,
            jobs_viewport: JobsViewport::default(),
            detail_viewport: DetailViewport::default(),
            pending_job: None,
            running_job_count,
            link_state,
            active_host: ActiveHost::Codex,
            dsh_status,
            paths,
            settings,
            provider_detection,
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
            dsh_plan: None,
            pending: None,
            retry_effect: None,
            last_jobs_refresh: Some(Instant::now()),
            last_tail_refresh: None,
            migration_notice_pending,
            startup_update_check_requested: false,
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

    pub(crate) fn config_messages(&self) -> &'static ConfigMessages {
        let language = if self.settings.language.is_none() {
            Language::En
        } else {
            self.language
        };
        config_i18n::messages(language)
    }

    pub(crate) fn guard_messages(&self) -> &'static GuardMessages {
        let language = if self.settings.language.is_none() {
            Language::En
        } else {
            self.language
        };
        guard_i18n::messages(language)
    }

    /// Whether the visible provider currently locks the effective output tier to Guarded.
    pub(crate) fn output_guard_active(&self) -> bool {
        self.config_draft.output_guard_enabled && self.provider_detection.requires_guard()
    }

    /// Why the visible provider activates Guarded copy, when it does.
    pub(crate) fn output_guard_reason(&self) -> Option<GuardReason> {
        self.provider_detection.guard_reason()
    }

    /// Concrete output policy shown by the configuration UI.
    pub(crate) fn effective_output(&self) -> EffectiveOutput {
        provider::effective_output(
            self.config_draft.output.tier,
            self.config_draft.output.budgets,
            self.config_draft.output_guard_enabled,
            &self.provider_detection,
        )
    }

    pub(crate) fn update_messages(&self) -> &'static UpdateMessages {
        let language = if self.settings.language.is_none() {
            Language::En
        } else {
            self.language
        };
        update_copy::messages(language)
    }

    pub(crate) fn migration_messages(&self) -> &'static MigrationMessages {
        let language = if self.settings.language.is_none() {
            Language::En
        } else {
            self.language
        };
        migration_copy::messages(language)
    }

    pub(crate) fn take_update_plan(&mut self) -> Option<UpdatePlan> {
        self.exit_update.take()
    }

    pub(crate) fn set_startup_update_check(&mut self, receiver: Receiver<StartupUpdate>) {
        self.update_check = Some(ActiveUpdateCheck {
            purpose: UpdateCheckPurpose::Startup,
            receiver,
            startup_deadline: None,
        });
        if self.settings.language.is_some() && !self.migration_notice_pending {
            self.enter_startup_update_gate();
        }
    }

    pub(crate) fn request_startup_update_check(&mut self) {
        self.startup_update_check_requested = true;
        if self.settings.language.is_some() {
            self.start_requested_startup_update_check();
        }
    }

    fn start_requested_startup_update_check(&mut self) {
        if !std::mem::take(&mut self.startup_update_check_requested) {
            return;
        }
        if let Some(receiver) = crate::update::spawn_startup_update_check(self.paths.clone()) {
            self.set_startup_update_check(receiver);
        }
    }

    pub(crate) fn poll_update_check(&mut self) {
        self.poll_update_check_at(Instant::now());
    }

    fn poll_update_check_at(&mut self, now: Instant) {
        let Some(check) = self.update_check.as_ref() else {
            return;
        };
        if check.purpose == UpdateCheckPurpose::Startup && self.screen != Screen::UpdateChecking {
            return;
        }
        let purpose = check.purpose;
        let result = match check.receiver.try_recv() {
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
        } else if purpose == UpdateCheckPurpose::Startup
            && check
                .startup_deadline
                .is_some_and(|deadline| now >= deadline)
        {
            // Dropping the receiver only detaches the UI. The worker still completes its
            // single bounded probe and commits any successful cache record.
            self.update_check = None;
            self.update_state = StartupUpdate::None;
            self.back_to_main();
        }
    }

    fn enter_startup_update_gate(&mut self) {
        let Some(check) = self.update_check.as_mut() else {
            return;
        };
        if check.purpose != UpdateCheckPurpose::Startup {
            return;
        }
        check.startup_deadline = Some(Instant::now() + STARTUP_UPDATE_GATE_TIMEOUT);
        self.screen = Screen::UpdateChecking;
        self.selected = 0;
        self.toast = None;
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
            Screen::MigrationNotice => self.handle_migration_notice(key.code),
            Screen::Update => self.handle_update(key.code),
            Screen::UpdateConfirm => self.handle_update_confirm(key.code),
            Screen::Language { first_run } => self.handle_language(key.code, first_run),
            Screen::Main => self.handle_main(key.code),
            Screen::Connections => self.handle_connections(key.code),
            Screen::ApplyHome => self.handle_apply_home(key.code),
            Screen::ApplyPreview => self.handle_apply_preview(key.code),
            Screen::ApplyConflict => {
                self.handle_binary_confirmation(key.code, Screen::ApplyConfirm)
            }
            Screen::ApplyConfirm => self.handle_apply_confirm(key.code),
            Screen::UnapplyPreview => self.handle_unapply_preview(key.code),
            Screen::UnapplyConfirm => self.handle_unapply_confirm(key.code),
            Screen::Config => self.handle_config(key),
            Screen::ConfigCpuEdit => self.handle_cpu_limit_editor(key),
            Screen::ConfigBudgetEdit(item) => self.handle_budget_editor(item, key),
            Screen::ConfigOutputGuardConfirm => self.handle_output_guard_confirm(key.code),
            Screen::ConfigDiscardConfirm => self.handle_config_discard_confirm(key.code),
            Screen::ConfigResetConfirm => self.handle_config_reset_confirm(key.code),
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
            | Screen::ConfigResetting
            | Screen::ConfigSaving
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
            Effect::CommitDshApply => Effect::PlanDshApply,
            Effect::CommitDshUnapply => Effect::PlanDshUnapply,
            effect => effect.clone(),
        };
        let is_doctor_effect = matches!(&effect, Effect::RunDoctor);
        let is_kill_effect = matches!(&effect, Effect::KillJob { .. });
        let result = match effect {
            Effect::RetryUpdate => {
                self.update_check = Some(ActiveUpdateCheck {
                    purpose: UpdateCheckPurpose::UpdatePage,
                    receiver: crate::update::spawn_update_check(self.paths.clone(), true),
                    startup_deadline: None,
                });
                Ok(())
            }
            Effect::SaveLanguage { first_run } => {
                let mut updated = self.settings.clone();
                updated.language = Some(self.language.code().to_string());
                self.save_settings(&updated).map(|_| {
                    self.settings = updated;
                    if first_run {
                        self.start_requested_startup_update_check();
                        if self.migration_notice_pending {
                            self.set_screen(Screen::MigrationNotice);
                            return;
                        }
                        if self
                            .update_check
                            .as_ref()
                            .is_some_and(|check| check.purpose == UpdateCheckPurpose::Startup)
                        {
                            if self.screen != Screen::UpdateChecking {
                                self.enter_startup_update_gate();
                            }
                            return;
                        }
                    }
                    self.screen = Screen::Main;
                    self.selected = 0;
                })
            }
            Effect::SaveConfig => {
                let mut updated = self.settings.clone();
                self.config_draft.apply_to(&mut updated);
                let guard_changed =
                    updated.output_guard.enabled != self.settings.output_guard.enabled;
                let extensions_changed =
                    updated.fastshell.enabled != self.settings.fastshell.enabled;
                let limits_changed = updated.fastshell.job_storage_limit_mib
                    != self.settings.fastshell.job_storage_limit_mib
                    || updated.fastshell.max_running_jobs
                        != self.settings.fastshell.max_running_jobs
                    || updated.fastshell.job_list_limit != self.settings.fastshell.job_list_limit;
                let search_changed =
                    updated.search.max_cpu_cores != self.settings.search.max_cpu_cores;
                let replace_limit_changed =
                    updated.replace.max_file_size_mib != self.settings.replace.max_file_size_mib;
                self.save_settings(&updated).map(|_| {
                    self.settings = updated;
                    // Saving is a step inside the settings page, not a way out of it: the draft
                    // is rebuilt from what actually landed on disk and the cursor stays put.
                    self.config_draft = self.saved_config_draft();
                    self.screen = Screen::Config;
                    let mut message = vec![self.messages().settings_saved];
                    if extensions_changed {
                        message.push(self.messages().extensions_note);
                    }
                    if limits_changed {
                        message.push(self.job_messages().user_limit_note);
                    }
                    if search_changed {
                        message.push(self.config_messages().cpu_limit_note);
                    }
                    if replace_limit_changed {
                        message.push(self.config_messages().replace_limit_saved_note);
                    }
                    if guard_changed {
                        message.push(if self.settings.output_guard.enabled {
                            self.guard_messages().available_note
                        } else {
                            self.guard_messages().disabled_note
                        });
                    }
                    self.toast = Some(Toast {
                        message: message.join("\n"),
                        warning: guard_changed && !self.settings.output_guard.enabled,
                    });
                })
            }
            Effect::ResetConfig => {
                let updated = settings::reset_user_preferences(&self.settings);
                let success = config_i18n::messages(Language::En)
                    .reset_success
                    .to_string();
                self.save_settings(&updated).map(|_| {
                    self.settings = updated;
                    self.config_draft = ConfigDraft::from_settings(&self.settings);
                    self.config_cursor = ConfigCursor::default();
                    self.config_viewport = ConfigViewport::default();
                    self.cpu_limit_editor = CpuLimitEditor::default();
                    self.budget_editor = BudgetEditor::default();
                    self.pending_job = None;
                    self.screen = Screen::Language { first_run: true };
                    self.selected = language_index(self.language);
                    self.toast = Some(Toast {
                        message: success,
                        warning: false,
                    });
                })
            }
            Effect::PlanApply => plan_apply(
                &self.paths,
                ApplyOptions {
                    tier: self.settings.tier,
                    tool_budgets: self.settings.tool_budgets,
                    output_guard_enabled: self.settings.output_guard.enabled,
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
                    self.settings.integrations.codex = None;
                    self.show_receipt(receipt);
                }),
            Effect::PlanDshApply => crate::control::dsh::plan_apply(
                &self.paths,
                crate::control::dsh::ApplyOptions {
                    tier: self.settings.tier,
                    tool_budgets: self.settings.tool_budgets,
                    fastshell_enabled: self.settings.fastshell.enabled,
                    current_executable: self.current_executable.clone(),
                },
            )
            .map(|plan| {
                self.dsh_plan = Some(plan);
                self.screen = Screen::ApplyPreview;
                self.selected = 0;
            }),
            Effect::CommitDshApply => self
                .dsh_plan
                .take()
                .ok_or_else(|| "The DeepSeek Harness Apply preview expired. Preview again.".to_string())
                .and_then(crate::control::dsh::commit_apply)
                .and_then(|changed| {
                    self.settings = settings::load(&self.paths)?;
                    self.dsh_status = crate::control::dsh::status(&self.paths);
                    self.show_receipt(OperationReceipt {
                        changed_targets: changed,
                        notes: vec!["DeepSeek Harness is connected host-wide. New sessions will use FastCtx.".to_string()],
                    });
                    Ok(())
                }),
            Effect::PlanDshUnapply => crate::control::dsh::plan_unapply(
                &self.paths,
                self.current_executable.clone(),
            )
            .map(|plan| {
                self.dsh_plan = Some(plan);
                self.screen = Screen::UnapplyPreview;
                self.selected = 0;
            }),
            Effect::CommitDshUnapply => self
                .dsh_plan
                .take()
                .ok_or_else(|| "The DeepSeek Harness Unapply preview expired. Preview again.".to_string())
                .and_then(crate::control::dsh::commit_unapply)
                .map(|changed| {
                    self.settings = settings::load(&self.paths).unwrap_or_default();
                    self.dsh_status = crate::control::dsh::status(&self.paths);
                    self.show_receipt(OperationReceipt {
                        changed_targets: changed,
                        notes: vec!["DeepSeek Harness was disconnected. Other host connections were preserved.".to_string()],
                    });
                }),
            Effect::PlanUnapplyAll => plan_unapply_all(
                &self.paths,
                self.current_executable.clone(),
            )
            .map(|plan| {
                self.unapply_plan = Some(plan);
                self.screen = Screen::UnapplyPreview;
                self.selected = 0;
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
            Effect::RunDshDoctor => {
                self.dsh_status = crate::control::dsh::status(&self.paths);
                self.screen = Screen::ApplyHome;
                self.selected = 2;
                self.toast = Some(Toast {
                    message: match &self.dsh_status {
                        Ok((state, detail)) => format!("DeepSeek Harness: {state}\n{detail}"),
                        Err(error) => format!("DeepSeek Harness status failed: {error}"),
                    },
                    warning: !matches!(&self.dsh_status, Ok((state, _)) if state == "connected"),
                });
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

    fn save_settings(&mut self, updated: &FastCtxSettings) -> Result<bool, String> {
        match settings::save(&self.paths, updated) {
            Ok(changed) => Ok(changed),
            Err(error) => {
                // The config replacement precedes terminal-job reaping. If only cleanup failed,
                // keep the in-memory model aligned with the already committed settings.
                if self.paths.fastctx_config.is_file()
                    && settings::load(&self.paths).is_ok_and(|persisted| persisted == *updated)
                {
                    self.settings = updated.clone();
                    self.config_draft = ConfigDraft::from_settings(&self.settings);
                }
                Err(error)
            }
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

    fn handle_migration_notice(&mut self, key: KeyCode) {
        if self.detail_viewport.handle_key(key) {
            return;
        }
        if !matches!(key, KeyCode::Enter | KeyCode::Esc) {
            return;
        }
        self.migration_notice_pending = false;
        if self
            .update_check
            .as_ref()
            .is_some_and(|check| check.purpose == UpdateCheckPurpose::Startup)
        {
            self.enter_startup_update_gate();
        } else {
            self.back_to_main();
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
                self.screen = Screen::Update;
                self.selected = 0;
                self.toast = None;
            }
            pending @ StartupUpdate::NpmPending { .. } => {
                self.update_state = pending;
                self.back_to_main();
            }
            current @ StartupUpdate::NpmCurrent { .. } => {
                self.update_state = current;
                self.back_to_main();
            }
            StartupUpdate::Failed(error) if error.kind == CheckFailureKind::Structural => {
                self.update_state = StartupUpdate::Failed(error.clone());
                self.screen = Screen::Main;
                self.selected = 0;
                self.toast = Some(Toast {
                    message: format!("{}: {}", self.update_messages().check_failed, error.message),
                    warning: true,
                });
            }
            StartupUpdate::InstallFailed(error) => {
                self.update_state = StartupUpdate::InstallFailed(error.clone());
                self.screen = Screen::Main;
                self.selected = 0;
                self.toast = Some(Toast {
                    message: format!("{}: {error}", self.update_messages().update_failed),
                    warning: true,
                });
            }
            failed @ StartupUpdate::Failed(_) => {
                self.update_state = failed;
                self.back_to_main();
            }
            StartupUpdate::None => {
                self.update_state = StartupUpdate::None;
                self.back_to_main();
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
                0 => self.set_screen(Screen::Connections),
                1 => {
                    self.provider_detection = provider::detect_path(&self.paths.codex_config);
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

    fn handle_connections(&mut self, key: KeyCode) {
        match key {
            KeyCode::Up | KeyCode::Char('k') => self.move_previous(3),
            KeyCode::Down | KeyCode::Char('j') => self.move_next(3),
            KeyCode::Enter => {
                self.active_host = match self.selected {
                    0 => ActiveHost::Codex,
                    1 => ActiveHost::DeepSeekHarness,
                    _ => ActiveHost::All,
                };
                if self.active_host == ActiveHost::All {
                    self.screen = Screen::UnapplyLoading;
                    self.pending = Some(Effect::PlanUnapplyAll);
                } else {
                    self.set_screen(Screen::ApplyHome);
                }
            }
            KeyCode::Esc => self.back_to_main(),
            _ => {}
        }
    }

    fn handle_apply_home(&mut self, key: KeyCode) {
        match key {
            KeyCode::Up | KeyCode::Char('k') => self.move_previous(3),
            KeyCode::Down | KeyCode::Char('j') => self.move_next(3),
            KeyCode::Enter if self.selected == 0 => {
                self.screen = Screen::ApplyLoading;
                self.pending = Some(match self.active_host {
                    ActiveHost::Codex => Effect::PlanApply,
                    ActiveHost::DeepSeekHarness => Effect::PlanDshApply,
                    ActiveHost::All => return,
                });
            }
            KeyCode::Enter if self.selected == 1 => {
                self.screen = Screen::UnapplyLoading;
                self.pending = Some(match self.active_host {
                    ActiveHost::Codex => Effect::PlanUnapply,
                    ActiveHost::DeepSeekHarness => Effect::PlanDshUnapply,
                    ActiveHost::All => Effect::PlanUnapplyAll,
                });
            }
            KeyCode::Enter => match self.active_host {
                ActiveHost::Codex => {
                    self.status = StatusState::Loading;
                    self.screen = Screen::Status;
                    self.pending = Some(Effect::RunDoctor);
                }
                ActiveHost::DeepSeekHarness => {
                    self.pending = Some(Effect::RunDshDoctor);
                }
                ActiveHost::All => {}
            },
            KeyCode::Esc => self.set_screen(Screen::Connections),
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
                self.dsh_plan = None;
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
                self.set_screen(if self.active_host == ActiveHost::All {
                    Screen::Connections
                } else {
                    Screen::ApplyHome
                });
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
                self.pending = Some(match self.active_host {
                    ActiveHost::Codex => Effect::CommitApply,
                    ActiveHost::DeepSeekHarness => Effect::CommitDshApply,
                    ActiveHost::All => return,
                });
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
                self.pending = Some(match self.active_host {
                    ActiveHost::Codex => Effect::CommitUnapply,
                    ActiveHost::DeepSeekHarness => Effect::CommitDshUnapply,
                    ActiveHost::All => Effect::CommitUnapply,
                });
            }
            KeyCode::Enter | KeyCode::Esc => {
                self.unapply_plan = None;
                self.set_screen(if self.active_host == ActiveHost::All {
                    Screen::Connections
                } else {
                    Screen::ApplyHome
                });
            }
            _ => {}
        }
    }

    fn handle_config(&mut self, key: KeyEvent) {
        let guarded = self.output_guard_active();
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.config_cursor = self.config_cursor.previous(),
            KeyCode::Down | KeyCode::Char('j') => self.config_cursor = self.config_cursor.next(),
            KeyCode::BackTab => self.config_cursor = self.config_cursor.previous_group(),
            KeyCode::Tab if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.config_cursor = self.config_cursor.previous_group()
            }
            KeyCode::Tab => self.config_cursor = self.config_cursor.next_group(),
            KeyCode::Left | KeyCode::Char('h') => {
                self.config_draft
                    .adjust_with_guard(self.config_cursor.entry().item, false, guarded)
            }
            KeyCode::Right | KeyCode::Char('l') => {
                self.config_draft
                    .adjust_with_guard(self.config_cursor.entry().item, true, guarded)
            }
            // Enter activates the focused item and nothing else: it opens an editor, raises a
            // confirmation, or advances the value. Writing the draft to disk is the separate,
            // explicit save below, so a single keystroke never commits settings the user only
            // meant to look at.
            KeyCode::Enter => match self.config_cursor.entry().item {
                ConfigItemId::OutputGuard if self.config_draft.output_guard_enabled => {
                    self.selected = 0;
                    self.screen = Screen::ConfigOutputGuardConfirm;
                }
                ConfigItemId::OutputGuard => self.config_draft.set_output_guard(true),
                ConfigItemId::SearchCpuLimit => {
                    self.cpu_limit_editor = CpuLimitEditor {
                        // An automatic limit opens on the count it currently resolves to, which
                        // is the number somebody adjusting it would otherwise have to look up.
                        input: self.config_draft.search_max_cpu_cores.map_or_else(
                            || search_parallelism::detected_available().to_string(),
                            |value| value.to_string(),
                        ),
                        error: None,
                    };
                    self.screen = Screen::ConfigCpuEdit;
                }
                item @ (ConfigItemId::ReadBudget
                | ConfigItemId::GrepBudget
                | ConfigItemId::GlobBudget
                | ConfigItemId::RunBudget
                | ConfigItemId::JobOutputBudget) => {
                    let ConfigValue::Budget(budget) =
                        self.config_draft.value_with_guard(item, guarded)
                    else {
                        return;
                    };
                    self.budget_editor = BudgetEditor {
                        // A share following the tier opens on the percentage it currently
                        // resolves to, so the field always starts from the value on screen.
                        input: budget.level.percent().to_string(),
                        error: None,
                    };
                    self.screen = Screen::ConfigBudgetEdit(item);
                }
                ConfigItemId::ResetAll => {
                    self.selected = 0;
                    self.screen = Screen::ConfigResetConfirm;
                }
                ConfigItemId::SaveAll if self.config_is_dirty() => self.begin_config_save(),
                // Pressing save with nothing pending says so rather than running a write that
                // would look identical to one that had something to do.
                ConfigItemId::SaveAll => {
                    self.toast = Some(Toast {
                        message: self.config_messages().save_button_clean.to_string(),
                        warning: false,
                    });
                }
                item => self.config_draft.adjust_with_guard(item, true, guarded),
            },
            KeyCode::Char('s') | KeyCode::Char('S') if self.config_is_dirty() => {
                self.begin_config_save();
            }
            KeyCode::Esc if self.config_is_dirty() => {
                self.selected = 0;
                self.screen = Screen::ConfigDiscardConfirm;
            }
            KeyCode::Esc => self.back_to_main(),
            _ => {}
        }
    }

    /// Puts the blocking save frame on screen before the write starts.
    ///
    /// `settings::save` runs synchronously on this thread and its trailing job reap can wait on a
    /// cross-process lock, so without a frame of its own the terminal would sit on an unchanged
    /// settings page for the whole duration with nothing to say the keystroke had registered.
    fn begin_config_save(&mut self) {
        self.screen = Screen::ConfigSaving;
        self.pending = Some(Effect::SaveConfig);
    }

    /// Whether the draft still holds edits that the saved settings do not have yet.
    pub(crate) fn config_is_dirty(&self) -> bool {
        self.config_draft != self.saved_config_draft()
    }

    /// How many individual items carry an edit that has not reached disk yet.
    pub(crate) fn config_unsaved_count(&self) -> usize {
        let saved = self.saved_config_draft();
        let guarded = self.output_guard_active();
        config::all_items()
            .filter(|item| self.config_draft.item_changed(saved, *item, guarded))
            .count()
    }

    /// The draft as it would look with no unsaved edits, for comparison against the live one.
    pub(crate) fn saved_config_draft(&self) -> ConfigDraft {
        ConfigDraft::from_settings(&self.settings)
    }

    fn handle_config_discard_confirm(&mut self, key: KeyCode) {
        match key {
            KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down => {
                self.selected = 1 - self.selected.min(1);
            }
            KeyCode::Enter if self.selected == 1 => {
                self.config_draft = self.saved_config_draft();
                self.selected = 0;
                self.back_to_main();
            }
            KeyCode::Enter | KeyCode::Esc => {
                self.selected = 0;
                self.screen = Screen::Config;
            }
            _ => {}
        }
    }

    fn handle_output_guard_confirm(&mut self, key: KeyCode) {
        match key {
            KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down => {
                self.selected = 1 - self.selected.min(1);
            }
            KeyCode::Enter if self.selected == 1 => {
                self.config_draft.set_output_guard(false);
                self.selected = 0;
                self.screen = Screen::Config;
            }
            KeyCode::Enter | KeyCode::Esc => {
                self.selected = 0;
                self.screen = Screen::Config;
            }
            _ => {}
        }
    }

    fn handle_cpu_limit_editor(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.cpu_limit_editor.error = None;
                self.screen = Screen::Config;
            }
            KeyCode::Enter => {
                let maximum = search_parallelism::detected_available();
                match search_parallelism::parse_input(&self.cpu_limit_editor.input, maximum) {
                    Ok(configured) => {
                        self.config_draft.set_search_cpu_limit(Some(configured));
                        self.cpu_limit_editor.error = None;
                        self.screen = Screen::Config;
                    }
                    Err(error) => self.cpu_limit_editor.error = Some(error),
                }
            }
            KeyCode::Backspace => {
                self.cpu_limit_editor.input.pop();
                self.cpu_limit_editor.error = None;
            }
            KeyCode::Delete => {
                self.cpu_limit_editor.input.clear();
                self.cpu_limit_editor.error = None;
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.cpu_limit_editor.input.clear();
                self.cpu_limit_editor.error = None;
            }
            // Digits only; see the budget editor for why a letter is refused as it is typed.
            KeyCode::Char(character)
                if character.is_ascii_digit()
                    && !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                    && self.cpu_limit_editor.input.chars().count() < 32 =>
            {
                self.cpu_limit_editor.input.push(character);
                self.cpu_limit_editor.error = None;
            }
            _ => {}
        }
    }

    fn handle_budget_editor(&mut self, item: ConfigItemId, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.budget_editor.error = None;
                self.screen = Screen::Config;
            }
            KeyCode::Enter => match budget_editor::parse_input(&self.budget_editor.input) {
                Ok(level) => {
                    self.config_draft.set_tool_budget(item, Some(level));
                    self.budget_editor.error = None;
                    self.screen = Screen::Config;
                }
                Err(error) => self.budget_editor.error = Some(error),
            },
            KeyCode::Backspace => {
                self.budget_editor.input.pop();
                self.budget_editor.error = None;
            }
            KeyCode::Delete => {
                self.budget_editor.input.clear();
                self.budget_editor.error = None;
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.budget_editor.input.clear();
                self.budget_editor.error = None;
            }
            // Only digits reach the field. The editor exists solely to land on a share the coarse
            // arrow-key stops skip over, so a letter here is always a mistake, and refusing it as
            // it is typed beats accepting it and reporting a validation failure on Enter.
            KeyCode::Char(character)
                if character.is_ascii_digit()
                    && !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                    && self.budget_editor.input.chars().count() < 32 =>
            {
                self.budget_editor.input.push(character);
                self.budget_editor.error = None;
            }
            _ => {}
        }
    }

    fn handle_config_reset_confirm(&mut self, key: KeyCode) {
        match key {
            KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down => {
                self.selected = 1 - self.selected.min(1);
            }
            KeyCode::Enter if self.selected == 1 => {
                self.screen = Screen::ConfigResetting;
                self.pending = Some(Effect::ResetConfig);
            }
            KeyCode::Enter | KeyCode::Esc => {
                self.selected = 0;
                self.screen = Screen::Config;
            }
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
                    Effect::PlanDshApply => Screen::ApplyLoading,
                    Effect::CommitDshApply => Screen::ApplyRunning,
                    Effect::PlanDshUnapply => Screen::UnapplyLoading,
                    Effect::CommitDshUnapply => Screen::UnapplyRunning,
                    Effect::PlanUnapplyAll => Screen::UnapplyLoading,
                    Effect::RunDoctor => {
                        self.status = StatusState::Loading;
                        Screen::Status
                    }
                    Effect::RunDshDoctor => Screen::ApplyHome,
                    Effect::SaveConfig => Screen::Config,
                    Effect::ResetConfig => Screen::ConfigResetting,
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
        // Apply and Unapply both reach this after refreshing the receipt, so it is the single
        // place the menu's connection state has to be recomputed.
        self.link_state = link::link_state(&self.paths, self.settings.integrations.codex.as_ref());
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
    use super::{App, Effect, Screen, config};
    use crate::control::i18n::Language;
    use crate::control::paths::ControlPaths;
    use crate::control::settings::{FastCtxSettings, Tier};

    use crate::tui::config::ConfigItemId;

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// Defaults as they look once this build has stamped them: both the software watermark and
    /// the budget-defaults epoch are bookkeeping rather than preferences, so a settings file this
    /// build has touched always carries them.
    fn default_with_current_watermark() -> FastCtxSettings {
        FastCtxSettings {
            last_seen_version: Some(env!("CARGO_PKG_VERSION").to_string()),
            tool_budget_epoch: Some(crate::control::settings::TOOL_BUDGET_EPOCH),
            ..FastCtxSettings::default()
        }
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

    #[test]
    fn first_run_requires_language_selection_before_main_menu() {
        let (_temp, mut app) = fixture();
        assert!(matches!(app.screen, Screen::Language { first_run: true }));
        app.handle_key(key(KeyCode::Enter));
        assert!(app.has_pending_effect());
        app.execute_pending();
        assert_eq!(app.screen, Screen::Main, "{:?}", app.error);
        assert!(app.settings.language.is_some());
    }

    #[test]
    fn apply_flow_reaches_preview_and_receipt_from_one_frozen_plan() {
        let (_temp, mut app) = fixture();
        app.settings.language = Some("en".to_string());
        app.screen = Screen::Main;
        app.selected = 0;
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.screen, Screen::Connections);
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
    fn connections_selects_deepseek_harness_and_runs_its_apply_flow() {
        let (_temp, mut app) = fixture();
        app.settings.language = Some("en".to_string());
        app.screen = Screen::Connections;
        app.selected = 1;
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.active_host, super::ActiveHost::DeepSeekHarness);
        assert_eq!(app.screen, Screen::ApplyHome);
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.screen, Screen::ApplyLoading);
        app.execute_pending();
        assert_eq!(app.screen, Screen::ApplyPreview, "{:?}", app.error);
        assert!(app.dsh_plan.is_some());
        assert!(app.apply_plan.is_none());
    }

    #[test]
    fn startup_restores_a_nondefault_dsh_home_from_the_apply_receipt() {
        let temp = tempfile::tempdir().unwrap();
        let default_paths = ControlPaths::for_home(temp.path());
        let dsh_home = temp.path().join("custom-dsh");
        let dsh_paths = ControlPaths::for_home_and_codex_home_and_dsh_home(
            temp.path(),
            &default_paths.codex_dir,
            default_paths.codex_home_source,
            Some(dsh_home.clone()),
        )
        .unwrap();
        let executable = temp.path().join(if cfg!(windows) {
            "source.exe"
        } else {
            "source"
        });
        std::fs::write(&executable, b"binary").unwrap();
        let plan = crate::control::dsh::plan_apply(
            &dsh_paths,
            crate::control::dsh::ApplyOptions {
                tier: Tier::Standard,
                tool_budgets: crate::control::settings::ToolBudgetPreferences::default(),
                fastshell_enabled: false,
                current_executable: executable,
            },
        )
        .unwrap();
        crate::control::dsh::commit_apply(plan).unwrap();
        let mut saved = crate::control::settings::load(&dsh_paths).unwrap();
        saved.installation = None;
        std::fs::write(
            &dsh_paths.fastctx_config,
            crate::control::settings::encode(&saved).unwrap(),
        )
        .unwrap();

        let app = App::load(default_paths).unwrap();

        assert_eq!(app.paths.dsh_dir, dsh_home);
        assert_eq!(
            app.paths.dsh_home_source,
            crate::control::paths::DshHomeSource::Receipt
        );
        assert!(matches!(&app.dsh_status, Ok((state, _)) if state == "unhealthy"));
    }

    #[test]
    fn disconnect_all_uses_a_separate_frozen_preview_and_cancel_is_zero_write() {
        let (temp, mut app) = fixture();
        app.settings.language = Some("en".to_string());
        app.screen = Screen::Connections;
        app.selected = 2;
        let before = file_tree(temp.path());
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.active_host, super::ActiveHost::All);
        assert_eq!(app.screen, Screen::UnapplyLoading);
        app.execute_pending();
        assert_eq!(app.screen, Screen::UnapplyPreview, "{:?}", app.error);
        assert!(app.unapply_plan.is_some());
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.screen, Screen::UnapplyConfirm);
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.screen, Screen::Connections);
        assert_eq!(file_tree(temp.path()), before);
    }

    #[test]
    fn tui_unapply_cancel_is_zero_write_then_confirm_restores_user_bytes() {
        let (temp, mut app) = fixture();
        // The shared limit already holds exactly what the default tier asks for, so Apply has no
        // conflict to confirm and Unapply has to hand these bytes back untouched.
        let config = format!(
            concat!(
                "# user config\n",
                "tool_output_token_limit = {} # exact\n",
                "\n",
                "[mcp_servers.other]\n",
                "command = 'other'\n",
            ),
            Tier::Standard.host_limit()
        );
        let agents = "# User rules\n\nKeep this exact.\n";
        std::fs::write(&app.paths.codex_config, &config).unwrap();
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
    fn reset_save_failure_keeps_context_and_retries_without_false_success() {
        let (_temp, mut app) = fixture();
        app.settings.language = Some("en".to_string());
        app.settings.tier = Tier::High;
        app.settings.search.max_cpu_cores = Some(1);
        app.config_draft = crate::tui::config::ConfigDraft::from_settings(&app.settings);
        let reset_success = crate::control::config_i18n::messages(Language::En).reset_success;
        std::fs::write(&app.paths.fastctx_dir, b"blocks directory creation").unwrap();
        app.screen = Screen::Config;
        app.config_cursor = config::cursor_for(ConfigItemId::ResetAll);
        app.handle_key(key(KeyCode::Enter));
        app.handle_key(key(KeyCode::Right));
        app.handle_key(key(KeyCode::Enter));
        app.execute_pending();

        assert_eq!(app.screen, Screen::OperationFailed);
        assert_eq!(app.settings.tier, Tier::High);
        assert_eq!(app.config_draft.output.tier, Tier::High);
        assert!(app.toast.is_none());
        assert!(matches!(app.retry_effect, Some(Effect::ResetConfig)));

        std::fs::remove_file(&app.paths.fastctx_dir).unwrap();
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.screen, Screen::ConfigResetting);
        app.execute_pending();
        assert_eq!(app.screen, Screen::Language { first_run: true });
        assert_eq!(app.settings, default_with_current_watermark());
        assert_eq!(
            app.toast.as_ref().map(|toast| toast.message.as_str()),
            Some(reset_success)
        );
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
