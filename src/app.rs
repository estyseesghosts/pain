//! Minimal Silvestris application: one custom-chrome window, one real PTY
//! session, one GPUI terminal renderer. `PAIN_FIXTURE=1` keeps the old canned
//! snapshot and `PAIN_STRESS=1` runs the synthetic producer for M2 tests.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{
    App, AppContext, Bounds, ClipboardItem, Entity, IntoElement, Render, SharedString, Window,
};
use silvestris::chrome::{open_window, window_options, WindowFrame};
use silvestris::core::command::CommandId;
use silvestris::responder::{ResponderChain, ResponderEntry, ResponderScope};
use silvestris::{ActiveTheme, ThemePreference};

use crate::config::{parse_args, usage, version_line};
use crate::log;
use crate::platform::{default_shell, spawn_pty, PlatformPty, PtyConfig};
use crate::terminal::fixtures::Fixture;
use crate::terminal::geometry::FONT_SIZE_STEP;
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
        SelectAllTerminal
    ]
);

const INCREASE_FONT: &str = "pain.increase-font";
const DECREASE_FONT: &str = "pain.decrease-font";
const RESET_FONT: &str = "pain.reset-font";
const COPY_SELECTION: &str = "pain.copy-selection";
const PASTE_CLIPBOARD: &str = "pain.paste-clipboard";
const SELECT_ALL: &str = "pain.select-all";

/// The only commands allowed out of a focused terminal canvas.
fn whitelisted_commands() -> [CommandId; 6] {
    [
        CommandId::new(INCREASE_FONT),
        CommandId::new(DECREASE_FONT),
        CommandId::new(RESET_FONT),
        CommandId::new(COPY_SELECTION),
        CommandId::new(PASTE_CLIPBOARD),
        CommandId::new(SELECT_ALL),
    ]
}

/// True when the focused responder does not claim this app command.
fn command_reaches_app(chain: &ResponderChain, id: &CommandId, window: &Window, cx: &App) -> bool {
    chain.resolve(id, window, cx).is_none()
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
    let command = config.shell.clone().unwrap_or_else(default_shell);
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

        // Capacity one: the session coalesces many VT writes into a single
        // token, and `try_send` drops redundant tokens instead of queueing a
        // per-chunk storm. The foreground task below does one `cx.notify`.
        let (wake_tx, wake_rx) = async_channel::bounded::<()>(1);
        let session_config = SessionConfig {
            cols: 80,
            rows: 24,
            scrollback_lines: config.scrollback_lines,
            scrollback_bytes: config.scrollback_bytes,
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
        // Terminal replies and OSC title changes are effect callbacks. They
        // never block on OS I/O or re-enter the terminal lock's writer path.
        let (title_tx, title_rx) = async_channel::bounded::<String>(1);
        session
            .install_callbacks(Box::new(move |title| {
                let _ = title_tx.try_send(title);
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
            // Silvestris responder policy: a focused terminal canvas captures
            // every command except the six app-owned ones. GPUI still owns key
            // dispatch; this chain decides which app commands may fire.
            let focus = terminal.read(cx).focus();
            let mut chain = ResponderChain::new();
            chain.push(
                ResponderEntry::custom(ResponderScope::Control, "terminal-canvas")
                    .when_focused(&focus)
                    .capture_keys(whitelisted_commands()),
            );
            cx.new(|_| AppView {
                terminal,
                pty: holder_for_view,
                title: SharedString::from("pain"),
                chain,
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
            gpui::KeyBinding::new("ctrl--", DecreaseFont, None),
            // Alternate spellings emitted by non-US and shifted layouts.
            gpui::KeyBinding::new("ctrl-minus", DecreaseFont, None),
            gpui::KeyBinding::new("ctrl-underscore", DecreaseFont, None),
            gpui::KeyBinding::new("ctrl-shift--", DecreaseFont, None),
            gpui::KeyBinding::new("ctrl-0", ResetFont, None),
            gpui::KeyBinding::new("ctrl-shift-c", CopySelection, None),
            gpui::KeyBinding::new("ctrl-shift-v", PasteClipboard, None),
            gpui::KeyBinding::new("ctrl-shift-a", SelectAllTerminal, None),
        ]);
        install_actions(cx, &view);

        let window_handle = view.clone();
        cx.spawn(async move |cx: &mut gpui::AsyncApp| {
            while wake_rx.recv().await.is_ok() {
                let _ = window_handle.update(cx, |_, _, cx| cx.notify());
            }
        })
        .detach();

        // OSC 0/2 title effects arrive on the PTY reader thread. A capacity-one
        // channel coalesces them and the UI task applies the latest title to
        // both the platform window and Silvestris's drawn title band.
        let title_window = view.clone();
        let title_session = Arc::clone(&session);
        cx.spawn(async move |cx: &mut gpui::AsyncApp| {
            while let Ok(new_title) = title_rx.recv().await {
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
        cx.on_window_closed(move |cx, _| {
            stop_on_close.store(true, Ordering::Release);
            // Close the writer queue so its thread can finish, then hand the
            // PTY to a background thread: shutdown joins workers and reaps the
            // child, which must never happen on the GPUI foreground thread.
            let holder = Arc::clone(&holder_on_close);
            std::thread::spawn(move || {
                if let Some(pty) = holder.lock().expect("pty holder poisoned").take() {
                    pty.shutdown();
                }
            });
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        cx.activate(true);
    });
}

/// Global commands are whitelisted so they never reach a PTY. Clipboard uses
/// the GPUI/platform clipboard; paste still goes through Ghostty's API.
fn install_actions(cx: &mut App, window: &gpui::WindowHandle<AppView>) {
    let w = window.clone();
    cx.on_action(move |_: &IncreaseFont, cx| {
        let _ = w.update(cx, |app, window, cx| {
            if !command_reaches_app(&app.chain, &CommandId::new(INCREASE_FONT), window, cx) {
                return;
            }
            app.terminal
                .update(cx, |terminal, cx| terminal.adjust_font(FONT_SIZE_STEP, cx));
        });
    });
    let w = window.clone();
    cx.on_action(move |_: &DecreaseFont, cx| {
        let _ = w.update(cx, |app, window, cx| {
            if !command_reaches_app(&app.chain, &CommandId::new(DECREASE_FONT), window, cx) {
                return;
            }
            app.terminal
                .update(cx, |terminal, cx| terminal.adjust_font(-FONT_SIZE_STEP, cx));
        });
    });
    let w = window.clone();
    cx.on_action(move |_: &ResetFont, cx| {
        let default_size = cx.theme().type_.size_mono;
        let _ = w.update(cx, |app, window, cx| {
            if !command_reaches_app(&app.chain, &CommandId::new(RESET_FONT), window, cx) {
                return;
            }
            app.terminal
                .update(cx, |terminal, cx| terminal.set_font_size(default_size, cx));
        });
    });
    let w = window.clone();
    cx.on_action(move |_: &CopySelection, cx| {
        let _ = w.update(cx, |app, window, cx| {
            if !command_reaches_app(&app.chain, &CommandId::new(COPY_SELECTION), window, cx) {
                return;
            }
            match app.terminal.read(cx).session().copy_selection() {
                Ok(text) if !text.is_empty() => {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                }
                Ok(_) => {}
                Err(error) => log::error("copy", &format!("copy failed: {error}")),
            }
        });
    });
    let w = window.clone();
    cx.on_action(move |_: &PasteClipboard, cx| {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        let _ = w.update(cx, |app, window, cx| {
            if !command_reaches_app(&app.chain, &CommandId::new(PASTE_CLIPBOARD), window, cx) {
                return;
            }
            if let Err(error) = app.terminal.read(cx).session().paste(&text) {
                log::error("paste", &format!("paste failed: {error}"));
            }
        });
    });
    let w = window.clone();
    cx.on_action(move |_: &SelectAllTerminal, cx| {
        let _ = w.update(cx, |app, window, cx| {
            if !command_reaches_app(&app.chain, &CommandId::new(SELECT_ALL), window, cx) {
                return;
            }
            if let Err(error) = app.terminal.read(cx).session().select_all() {
                log::error("selection", &format!("select all failed: {error}"));
            }
        });
    });
}

/// Root view: Silvestris draws the frame; the terminal is the domain child.
struct AppView {
    terminal: Entity<TerminalView>,
    #[allow(dead_code)]
    pty: Arc<Mutex<Option<Box<dyn PlatformPty>>>>,
    title: SharedString,
    chain: ResponderChain,
}

impl Render for AppView {
    fn render(&mut self, _window: &mut Window, _cx: &mut gpui::Context<Self>) -> impl IntoElement {
        WindowFrame::new(self.title.clone()).child(self.terminal.clone())
    }
}

/// Synthetic byte producer: exactly the entry point a ConPTY/POSIX reader will
/// use, but with fixture chunks instead of OS output.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn responder_allowlist_is_exactly_the_app_commands() {
        let allowed = whitelisted_commands();
        assert_eq!(allowed.len(), 6);
        for expected in [
            INCREASE_FONT,
            DECREASE_FONT,
            RESET_FONT,
            COPY_SELECTION,
            PASTE_CLIPBOARD,
            SELECT_ALL,
        ] {
            assert!(allowed.iter().any(|id| id.as_str() == expected));
        }
        assert!(!allowed.iter().any(|id| id.as_str() == "edit.paste"));
    }
}

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
