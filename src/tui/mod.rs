//! Full-screen ratatui control terminal.

mod app;
mod config;
mod jobs;
mod theme;
mod update;
mod view;

use crate::control::paths::ControlPaths;
use crossterm::cursor::{Hide, Show};
use crossterm::event::{self, Event, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use std::io;
use std::panic::{self, AssertUnwindSafe, PanicHookInfo};
use std::sync::Arc;
use std::time::Duration;

pub(crate) enum TuiOutcome {
    Exit,
    Update(Box<crate::update::UpdatePlan>),
}

/// Starts the full-screen TUI and restores the terminal on every exit path.
pub(crate) fn run(
    paths: ControlPaths,
    startup_update: crate::update::StartupUpdate,
    startup_notice: Option<crate::update::FinalizeNotice>,
    startup_check: Option<std::sync::mpsc::Receiver<crate::update::StartupUpdate>>,
) -> Result<TuiOutcome, String> {
    let panic_hook = PanicHookGuard::install();
    let terminal_guard = TerminalGuard::enter(CrosstermControl)?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = match Terminal::new(backend) {
        Ok(terminal) => terminal,
        Err(error) => return Err(format!("Cannot initialize the terminal UI: {error}")),
    };

    let outcome = panic::catch_unwind(AssertUnwindSafe(|| {
        run_loop(
            &mut terminal,
            paths,
            startup_update,
            startup_notice,
            startup_check,
        )
    }));
    drop(terminal);
    drop(terminal_guard);
    drop(panic_hook);
    match outcome {
        Ok(result) => result,
        Err(payload) => panic::resume_unwind(payload),
    }
}

fn restore_terminal_unconditionally() {
    let mut stdout = io::stdout();
    let _ = execute!(stdout, Show, LeaveAlternateScreen);
    let _ = disable_raw_mode();
}

trait TerminalControl {
    fn enable_raw_mode(&mut self) -> io::Result<()>;
    fn disable_raw_mode(&mut self) -> io::Result<()>;
    fn enter_alternate_screen(&mut self) -> io::Result<()>;
    fn leave_alternate_screen(&mut self) -> io::Result<()>;
    fn hide_cursor(&mut self) -> io::Result<()>;
    fn show_cursor(&mut self) -> io::Result<()>;
}

struct CrosstermControl;

impl TerminalControl for CrosstermControl {
    fn enable_raw_mode(&mut self) -> io::Result<()> {
        enable_raw_mode()
    }

    fn disable_raw_mode(&mut self) -> io::Result<()> {
        disable_raw_mode()
    }

    fn enter_alternate_screen(&mut self) -> io::Result<()> {
        execute!(io::stdout(), EnterAlternateScreen)
    }

    fn leave_alternate_screen(&mut self) -> io::Result<()> {
        execute!(io::stdout(), LeaveAlternateScreen)
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        execute!(io::stdout(), Hide)
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        execute!(io::stdout(), Show)
    }
}

struct TerminalGuard<C: TerminalControl> {
    control: C,
    raw_mode: bool,
    alternate_screen: bool,
    cursor_hidden: bool,
}

impl<C: TerminalControl> TerminalGuard<C> {
    fn enter(control: C) -> Result<Self, String> {
        let mut guard = Self {
            control,
            raw_mode: false,
            alternate_screen: false,
            cursor_hidden: false,
        };
        guard
            .control
            .enable_raw_mode()
            .map_err(|error| format!("Cannot enable terminal raw mode: {error}"))?;
        guard.raw_mode = true;
        guard
            .control
            .enter_alternate_screen()
            .map_err(|error| format!("Cannot enter the alternate terminal screen: {error}"))?;
        guard.alternate_screen = true;
        guard
            .control
            .hide_cursor()
            .map_err(|error| format!("Cannot hide the terminal cursor: {error}"))?;
        guard.cursor_hidden = true;
        Ok(guard)
    }
}

impl<C: TerminalControl> Drop for TerminalGuard<C> {
    fn drop(&mut self) {
        if self.cursor_hidden {
            let _ = self.control.show_cursor();
            self.cursor_hidden = false;
        }
        if self.alternate_screen {
            let _ = self.control.leave_alternate_screen();
            self.alternate_screen = false;
        }
        if self.raw_mode {
            let _ = self.control.disable_raw_mode();
            self.raw_mode = false;
        }
    }
}

type PanicHook = dyn for<'a> Fn(&PanicHookInfo<'a>) + Send + Sync + 'static;

struct PanicHookGuard {
    previous: Arc<PanicHook>,
}

impl PanicHookGuard {
    fn install() -> Self {
        let previous: Arc<PanicHook> = Arc::from(panic::take_hook());
        let chained = Arc::clone(&previous);
        panic::set_hook(Box::new(move |info| {
            // The hook runs before unwinding, so cleanup happens before the prior hook prints the panic.
            restore_terminal_unconditionally();
            chained(info);
        }));
        Self { previous }
    }
}

impl Drop for PanicHookGuard {
    fn drop(&mut self) {
        let previous = Arc::clone(&self.previous);
        panic::set_hook(Box::new(move |info| previous(info)));
    }
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    paths: ControlPaths,
    startup_update: crate::update::StartupUpdate,
    startup_notice: Option<crate::update::FinalizeNotice>,
    startup_check: Option<std::sync::mpsc::Receiver<crate::update::StartupUpdate>>,
) -> Result<TuiOutcome, String> {
    let mut app = app::App::load_with_startup(paths, startup_update, startup_notice)?;
    if let Some(receiver) = startup_check {
        app.set_startup_update_check(receiver);
    }
    loop {
        app.tick();
        terminal
            .draw(|frame| view::render(frame, &mut app))
            .map_err(|error| format!("Cannot draw the terminal UI: {error}"))?;
        // Poll only after drawing so even an immediate cache hit cannot replace the first frame.
        app.poll_update_check();
        if app.should_quit {
            return Ok(match app.take_update_plan() {
                Some(plan) => TuiOutcome::Update(Box::new(plan)),
                None => TuiOutcome::Exit,
            });
        }
        if app.has_pending_effect() {
            app.execute_pending();
            continue;
        }
        if !event::poll(Duration::from_millis(100))
            .map_err(|error| format!("Cannot poll terminal events: {error}"))?
        {
            continue;
        }
        match event::read().map_err(|error| format!("Cannot read a terminal event: {error}"))? {
            Event::Key(key) if key.kind == KeyEventKind::Press => app.handle_key(key),
            Event::Resize(_, _) => {}
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{TerminalControl, TerminalGuard};
    use std::cell::RefCell;
    use std::io;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::rc::Rc;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Action {
        EnableRaw,
        EnterAlternate,
        HideCursor,
        ShowCursor,
        LeaveAlternate,
        DisableRaw,
    }

    #[derive(Clone)]
    struct FakeControl(Rc<RefCell<Vec<Action>>>);

    impl FakeControl {
        fn push(&self, action: Action) {
            self.0.borrow_mut().push(action);
        }
    }

    impl TerminalControl for FakeControl {
        fn enable_raw_mode(&mut self) -> io::Result<()> {
            self.push(Action::EnableRaw);
            Ok(())
        }

        fn disable_raw_mode(&mut self) -> io::Result<()> {
            self.push(Action::DisableRaw);
            Ok(())
        }

        fn enter_alternate_screen(&mut self) -> io::Result<()> {
            self.push(Action::EnterAlternate);
            Ok(())
        }

        fn leave_alternate_screen(&mut self) -> io::Result<()> {
            self.push(Action::LeaveAlternate);
            Ok(())
        }

        fn hide_cursor(&mut self) -> io::Result<()> {
            self.push(Action::HideCursor);
            Ok(())
        }

        fn show_cursor(&mut self) -> io::Result<()> {
            self.push(Action::ShowCursor);
            Ok(())
        }
    }

    #[test]
    fn terminal_guard_restores_cursor_alternate_screen_and_raw_mode_during_unwind() {
        let actions = Rc::new(RefCell::new(Vec::new()));
        let result = catch_unwind(AssertUnwindSafe({
            let actions = Rc::clone(&actions);
            move || {
                let _guard = TerminalGuard::enter(FakeControl(actions)).unwrap();
                panic!("injected TUI panic");
            }
        }));
        assert!(result.is_err());
        assert_eq!(
            *actions.borrow(),
            [
                Action::EnableRaw,
                Action::EnterAlternate,
                Action::HideCursor,
                Action::ShowCursor,
                Action::LeaveAlternate,
                Action::DisableRaw,
            ]
        );
    }
}
