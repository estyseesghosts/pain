//! Minimal Silvestris application: one custom-chrome window, one real PTY
//! session, one GPUI terminal renderer. `PAIN_FIXTURE=1` keeps the old canned
//! snapshot and `PAIN_STRESS=1` runs the synthetic producer for M2 tests.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{App, AppContext, Bounds, Entity, IntoElement, Render, SharedString, Window};
use silvestris::chrome::{open_window, window_options, WindowFrame};
use silvestris::ThemePreference;

use crate::config::{parse_args, usage, version_line, Config};
use crate::log;
use crate::platform::profiles::{self, ShellProfile};
use crate::platform::{default_shell, spawn_pty, PlatformPty, PtyConfig, ShellCommand};
use crate::preferences::{self, SessionGlobal, ShellProfilesGlobal};
use crate::terminal::fixtures::Fixture;
use crate::terminal::render::TerminalView;
use crate::terminal::session::{SessionConfig, TerminalSession};

// Deliberately whitelisted app commands. Every other key reaches the terminal.
gpui::actions!(
    pain,
    [
        IncreaseFont,
        DecreaseFont,
        ResetFont,
        CopySelection,
        PasteClipboard,
        SelectAllTerminal,
        OpenPreferences
    ]
);

/// Latest-value title mailbox. OSC 0/2 can produce A, B, C before the UI
/// consumes any of them; a capacity-one channel that drops on send can lose C,
/// so the mailbox keeps only the newest value and one coalesced notification.
struct TitleMailbox {
    latest: Mutex<Option<String>>,
    queued: AtomicBool,
    wake: async_channel::Sender<()>,
}

impl TitleMailbox {
    fn new(wake: async_channel::Sender<()>) -> Self {
        Self {
            latest: Mutex::new(None),
            queued: AtomicBool::new(false),
            wake,
        }
    }

    fn post(&self, title: String) {
        // Both operations happen under one lock so a post racing a consume
        // cannot be lost when the queued flag is cleared.
        let mut latest = self.latest.lock().expect("title mailbox poisoned");
        *latest = Some(title);
        if !self.queued.swap(true, Ordering::AcqRel) {
            let _ = self.wake.try_send(());
        }
    }

    fn take(&self) -> Option<String> {
        let mut latest = self.latest.lock().expect("title mailbox poisoned");
        let value = latest.take();
        if value.is_some() {
            self.queued.store(false, Ordering::Release);
        }
        value
    }
}

/// Start the one-window app with a live shell by default.
pub fn run() {
    let argv: Vec<String> = std::env::args().collect();
    let config = match parse_args(&argv) {
        Ok(config) => config,
        Err(error) => {
            log::error("cli", &error);
            println!("pain: {error}\n{}", usage());
            return;
        }
    };
    if config.help {
        log::info("cli", usage());
        println!("{}", usage());
        return;
    }
    if config.version {
        let version = version_line();
        log::info("cli", &version);
        println!("{version}");
        return;
    }
    let profiles = profiles::detect_profiles();
    let fixture = std::env::var_os("PAIN_FIXTURE").is_some();
    let stress = std::env::var_os("PAIN_STRESS").is_some();
    log::info(
        "startup",
        &format!(
            "{}; scrollback {} lines / {} bytes",
            version_line(),
            config.scrollback_lines,
            config.scrollback_bytes
        ),
    );

    gpui_platform::application().run(move |cx: &mut App| {
        if let Err(error) = silvestris::init(ThemePreference::default(), cx) {
            log::error("startup", &format!("silvestris init failed: {error:#}"));
            cx.quit();
            return;
        }
        // GPUI's own jump-list implementation relies on the process's implicit
        // AppUserModelID, so do not set an explicit one: an explicit id would
        // detach the running button from an existing taskbar pin.
        #[cfg(target_os = "windows")]
        profiles::install_jump_list(&profiles);
        cx.set_global(ShellProfilesGlobal(profiles.clone()));

        // Load the persisted appearance before any window opens. A malformed
        // document falls back to the previous look instead of blocking startup.
        preferences::install(cx);
        let saved_profile = cx
            .global::<preferences::PreferencesGlobal>()
            .0
            .read(cx)
            .value()
            .profile
            .clone();
        let command = choose_shell(&config, saved_profile.as_deref(), &profiles);
        log::info("startup", &format!("shell: {}", command.program));

        // Capacity one: the session coalesces many VT writes into a single
        // token, and `try_send` drops redundant tokens instead of queueing a
        // per-chunk storm. The foreground task below does one `cx.notify`.
        let (wake_tx, wake_rx) = async_channel::bounded::<()>(1);
        let session_config = SessionConfig {
            cols: 80,
            rows: 24,
            scrollback_lines: config.scrollback_lines,
            scrollback_bytes: config.scrollback_bytes,
            ..SessionConfig::default()
        };
        let session = Arc::new(
            TerminalSession::new_with_config(
                session_config,
                Box::new(move || {
                    let _ = wake_tx.try_send(());
                }),
            )
            .expect("terminal session created"),
        );
        cx.set_global(SessionGlobal(Arc::clone(&session)));
        preferences::apply(cx);
        // Terminal replies and OSC title changes are effect callbacks. They
        // never block on OS I/O or re-enter the terminal lock's writer path.
        let (title_tx, title_rx) = async_channel::bounded::<()>(1);
        let titles = Arc::new(TitleMailbox::new(title_tx));
        let title_sink = Arc::clone(&titles);
        session
            .install_callbacks(Box::new(move |title| {
                title_sink.post(title);
            }))
            .expect("install terminal effect callbacks");

        let stop = Arc::new(AtomicBool::new(false));
        let pty_holder: Arc<Mutex<Option<Box<dyn PlatformPty>>>> = Arc::new(Mutex::new(None));

        if !fixture && !stress {
            let pty_config = PtyConfig {
                cols: 80,
                rows: 24,
                command,
            };
            let pty = match spawn_pty(&pty_config, Arc::clone(&session)) {
                Ok(pty) => pty,
                Err(error) => {
                    log::error("shell", &format!("could not start shell: {error}"));
                    cx.quit();
                    return;
                }
            };
            // Pixel-only resizes never reach the PTY: the session invokes this
            // hook only after the Ghostty grid changes.
            let resize_holder = Arc::clone(&pty_holder);
            session.set_pty_resize(Arc::new(move |cols, rows| {
                if let Some(pty) = resize_holder.lock().expect("pty holder poisoned").as_ref() {
                    if let Err(error) = pty.resize(cols, rows) {
                        log::error("resize", &format!("PTY resize failed: {error}"));
                    }
                }
            }));
            *pty_holder.lock().expect("pty holder poisoned") = Some(pty);
        } else {
            Fixture::WideAndStyles.apply_session(&session);
            if !stress {
                // The first frame renders this fixture unconditionally from an
                // empty cache, so discard its initial wakeup here.
                let _ = session.take_wakeup();
            } else {
                start_producer(Arc::clone(&session), Arc::clone(&stop));
            }
        }

        let bounds = Bounds::centered(None, gpui::size(gpui::px(1040.), gpui::px(660.)), cx);
        let options = window_options(
            "pain",
            bounds,
            gpui::size(gpui::px(480.), gpui::px(320.)),
            cx,
        );
        let holder_for_view = Arc::clone(&pty_holder);
        let view = open_window(options, cx, |window: &mut Window, cx| {
            let terminal = cx.new(|cx| TerminalView::new(Arc::clone(&session), window, cx));
            cx.new(|_| AppView {
                terminal,
                pty: holder_for_view,
                title: SharedString::from("pain"),
            })
        })
        .expect("open terminal window");

        if let Some(size) = config.font_size {
            let _ = view.update(cx, |app, _, cx| {
                app.terminal
                    .update(cx, |terminal, cx| terminal.set_font_size(size, cx));
            });
        }

        cx.bind_keys([
            gpui::KeyBinding::new("ctrl-=", IncreaseFont, None),
            gpui::KeyBinding::new("ctrl-shift-=", IncreaseFont, None),
            // A shifted `=`/`-` is reported as `+`/`_` with Shift cleared on
            // Windows, so bind the produced characters as well.
            gpui::KeyBinding::new("ctrl-+", IncreaseFont, None),
            gpui::KeyBinding::new("ctrl--", DecreaseFont, None),
            gpui::KeyBinding::new("ctrl-_", DecreaseFont, None),
            // Alternate spellings emitted by non-US and shifted layouts.
            gpui::KeyBinding::new("ctrl-minus", DecreaseFont, None),
            gpui::KeyBinding::new("ctrl-underscore", DecreaseFont, None),
            gpui::KeyBinding::new("ctrl-shift--", DecreaseFont, None),
            gpui::KeyBinding::new("ctrl-0", ResetFont, None),
            gpui::KeyBinding::new("ctrl-shift-c", CopySelection, None),
            gpui::KeyBinding::new("ctrl-shift-v", PasteClipboard, None),
            // Shift+Insert is the portable paste binding on both Windows and
            // Linux (Zed's pinned terminal keymaps use it too). Ctrl+V stays a
            // terminal control byte: shells use it for quoted-insert.
            gpui::KeyBinding::new("shift-insert", PasteClipboard, None),
            gpui::KeyBinding::new("ctrl-shift-a", SelectAllTerminal, None),
            gpui::KeyBinding::new("ctrl-,", OpenPreferences, None),
        ]);

        // Early raw-keystroke ownership check. Windows marks many Ctrl keys as
        // `prefer_character_input`, which normally skips bindings and sends the
        // keystroke to the native text input path. The interceptor runs before
        // that decision, uses the SAME pure `app_command` allowlist as the
        // view's ownership check, and claims the key without calling back into
        // a window that GPUI has already leased for key dispatch. The actual
        // command is dispatched on the focused view after this dispatch ends.
        cx.intercept_keystrokes(move |event: &gpui::KeystrokeEvent, window, cx| {
            let Some(focus) = cx.try_global::<crate::terminal::render::TerminalFocusGlobal>()
            else {
                return;
            };
            if !focus.0.is_focused(window) {
                return;
            }
            let m = event.keystroke.modifiers;
            let mods =
                crate::terminal::input::Modifiers::new(m.shift, m.control, m.alt, m.platform);
            let Some(command) = crate::terminal::input::app_command(
                &event.keystroke.key,
                event.keystroke.key_char.as_deref(),
                mods,
            ) else {
                return;
            };
            cx.stop_propagation();
            let handle = window.window_handle();
            cx.defer(move |cx| {
                let action: Box<dyn gpui::Action> = match command {
                    crate::terminal::input::AppCommand::IncreaseFont => Box::new(IncreaseFont),
                    crate::terminal::input::AppCommand::DecreaseFont => Box::new(DecreaseFont),
                    crate::terminal::input::AppCommand::ResetFont => Box::new(ResetFont),
                    crate::terminal::input::AppCommand::CopySelection => Box::new(CopySelection),
                    crate::terminal::input::AppCommand::PasteClipboard => Box::new(PasteClipboard),
                    crate::terminal::input::AppCommand::SelectAll => Box::new(SelectAllTerminal),
                    crate::terminal::input::AppCommand::OpenPreferences => {
                        Box::new(OpenPreferences)
                    }
                };
                let _ = handle.update(cx, |_, window, cx| window.dispatch_action(action, cx));
            });
        })
        .detach();

        let window_handle = view;
        cx.spawn(async move |cx: &mut gpui::AsyncApp| {
            while wake_rx.recv().await.is_ok() {
                let _ = window_handle.update(cx, |_, _, cx| cx.notify());
            }
        })
        .detach();

        // OSC 0/2 title effects arrive on the PTY reader thread. A capacity-one
        // channel coalesces them and the UI task applies the latest title to
        // both the platform window and Silvestris's drawn title band.
        let title_window = view;
        let title_session = Arc::clone(&session);
        cx.spawn(async move |cx: &mut gpui::AsyncApp| {
            while title_rx.recv().await.is_ok() {
                let Some(new_title) = titles.take() else {
                    continue;
                };
                title_session.record_title_change();
                let display = if new_title.is_empty() {
                    "pain".to_owned()
                } else {
                    new_title
                };
                let _ = title_window.update(cx, |app, window, cx| {
                    app.title = SharedString::from(display.clone());
                    window.set_window_title(&display);
                    cx.notify();
                });
            }
        })
        .detach();

        let stop_on_close = Arc::clone(&stop);
        let holder_on_close = Arc::clone(&pty_holder);
        let terminal_window_id = view.window_id();
        cx.on_window_closed(move |cx, window| {
            // The preferences window is independent: closing it must not kill
            // the shell, and closing the terminal must leave prefs usable.
            if window == terminal_window_id {
                stop_on_close.store(true, Ordering::Release);
                // Close the writer queue so its thread can finish, then hand
                // the PTY to a background thread: shutdown joins workers and
                // reaps the child, which must never happen on the GPUI thread.
                let holder = Arc::clone(&holder_on_close);
                std::thread::spawn(move || {
                    if let Some(pty) = holder.lock().expect("pty holder poisoned").take() {
                        pty.shutdown();
                    }
                });
            }
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        cx.activate(true);
    });
}

/// Resolve launch precedence: explicit command, explicit profile, saved
/// profile, then the platform default (PowerShell 7 on Windows when present).
fn choose_shell(
    config: &Config,
    saved_profile: Option<&str>,
    profiles: &[ShellProfile],
) -> ShellCommand {
    if let Some(shell) = &config.shell {
        return shell.clone();
    }
    if let Some(id) = config.profile.as_deref() {
        match profiles::find_profile(profiles, id) {
            Some(profile) => return profile.command.clone(),
            None => log::error("cli", &format!("unknown shell profile: {id}")),
        }
    }
    if let Some(id) = saved_profile {
        match profiles::find_profile(profiles, id) {
            Some(profile) => return profile.command.clone(),
            None => log::error("startup", &format!("saved shell profile not found: {id}")),
        }
    }
    profiles::default_profile_id(profiles)
        .and_then(|id| profiles::find_profile(profiles, id))
        .map(|profile| profile.command.clone())
        .unwrap_or_else(default_shell)
}

/// Root view: Silvestris draws the frame; the terminal is the domain child.
struct AppView {
    terminal: Entity<TerminalView>,
    #[allow(dead_code)]
    pty: Arc<Mutex<Option<Box<dyn PlatformPty>>>>,
    title: SharedString,
}

impl Render for AppView {
    fn render(&mut self, _window: &mut Window, _cx: &mut gpui::Context<Self>) -> impl IntoElement {
        WindowFrame::new(self.title.clone()).child(self.terminal.clone())
    }
}

/// Synthetic byte producer: exactly the entry point a ConPTY/POSIX reader will
/// use, but with fixture chunks instead of OS output.
fn start_producer(session: Arc<TerminalSession>, stop: Arc<AtomicBool>) {
    std::thread::Builder::new()
        .name("pain-fixture-producer".into())
        .spawn(move || {
            let chunks = Fixture::WideAndStyles.chunks();
            let mut index = 0usize;
            while !stop.load(Ordering::Acquire) {
                session.write_vt(chunks[index % chunks.len()]);
                index += 1;
                std::thread::sleep(Duration::from_millis(2));
            }
        })
        .expect("spawn synthetic fixture producer");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(id: &str, program: &str) -> ShellProfile {
        ShellProfile {
            id: id.to_owned(),
            label: id.to_owned(),
            command: ShellCommand::new(program),
        }
    }

    #[test]
    fn title_mailbox_is_latest_wins_and_notifies_once() {
        let (tx, rx) = async_channel::bounded::<()>(1);
        let mailbox = TitleMailbox::new(tx);
        assert!(mailbox.take().is_none());
        mailbox.post("A".into());
        mailbox.post("B".into());
        mailbox.post("C".into());
        assert_eq!(rx.len(), 1, "coalesce notifications, do not queue titles");
        assert_eq!(mailbox.take().as_deref(), Some("C"));
        assert!(!mailbox.queued.load(Ordering::Acquire));
        assert!(mailbox.take().is_none());
        mailbox.post("D".into());
        assert_eq!(rx.len(), 1, "a later post re-arms the consumer");
        assert_eq!(mailbox.take().as_deref(), Some("D"));
    }

    #[test]
    fn launch_prefers_explicit_shell_then_profile_then_saved_then_default() {
        let profiles = vec![profile("cmd", "cmd.exe"), profile("pwsh7", "pwsh.exe")];
        let explicit = Config {
            shell: Some(ShellCommand::new("git-bash.exe")),
            profile: Some("pwsh7".to_owned()),
            ..Config::default()
        };
        assert_eq!(
            choose_shell(&explicit, Some("cmd"), &profiles).program,
            "git-bash.exe"
        );

        let by_profile = Config {
            profile: Some("pwsh7".to_owned()),
            ..Config::default()
        };
        assert_eq!(
            choose_shell(&by_profile, Some("cmd"), &profiles).program,
            "pwsh.exe"
        );

        let by_saved = Config::default();
        assert_eq!(
            choose_shell(&by_saved, Some("cmd"), &profiles).program,
            "cmd.exe"
        );

        let fallback = Config::default();
        #[cfg(target_os = "windows")]
        assert_eq!(choose_shell(&fallback, None, &profiles).program, "pwsh.exe");
        #[cfg(target_os = "linux")]
        assert_eq!(choose_shell(&fallback, None, &profiles).program, "cmd.exe");
    }
}
