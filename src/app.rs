//! Minimal Silvestris application: one custom-chrome window, one real PTY
//! session, one GPUI terminal renderer. `PAIN_FIXTURE=1` keeps the old canned
//! snapshot and `PAIN_STRESS=1` runs the synthetic producer for M2 tests.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{App, AppContext, Bounds, ClipboardItem, Entity, IntoElement, Render, Window};
use silvestris::chrome::{open_window, window_options, WindowFrame};
use silvestris::{ActiveTheme, ThemePreference};

use crate::platform::{
    default_shell, shell_from_args, spawn_pty, PtyConfig, PlatformPty,
};
use crate::terminal::fixtures::Fixture;
use crate::terminal::geometry::FONT_SIZE_STEP;
use crate::terminal::render::TerminalView;
use crate::terminal::session::TerminalSession;

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

/// Start the one-window app with a live shell by default.
pub fn run() {
    let argv: Vec<String> = std::env::args().collect();
    let command = shell_from_args(&argv).unwrap_or_else(default_shell);
    let fixture = std::env::var_os("PAIN_FIXTURE").is_some();
    let stress = std::env::var_os("PAIN_STRESS").is_some();

    gpui_platform::application().run(move |cx: &mut App| {
        if let Err(error) = silvestris::init(ThemePreference::default(), cx) {
            eprintln!("silvestris init failed: {error:#}");
            cx.quit();
            return;
        }

        // Capacity one: the session coalesces many VT writes into a single
        // token, and `try_send` drops redundant tokens instead of queueing a
        // per-chunk storm. The foreground task below does one `cx.notify`.
        let (wake_tx, wake_rx) = async_channel::bounded::<()>(1);
        let session = Arc::new(
            TerminalSession::new(80, 24, Box::new(move || {
                let _ = wake_tx.try_send(());
            }))
            .expect("terminal session created"),
        );
        // Terminal replies (device reports, mode queries) must never block the
        // terminal lock; this pushes them into the ordered writer queue.
        session
            .install_pty_writer()
            .expect("install WRITE_PTY bridge");

        let stop = Arc::new(AtomicBool::new(false));
        let pty_holder: Arc<Mutex<Option<Box<dyn PlatformPty>>>> = Arc::new(Mutex::new(None));

        if !fixture && !stress {
            let config = PtyConfig {
                cols: 80,
                rows: 24,
                command,
            };
            let pty = match spawn_pty(&config, Arc::clone(&session)) {
                Ok(pty) => pty,
                Err(error) => {
                    eprintln!("pain: could not start shell: {error}");
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
                        eprintln!("pain: PTY resize failed: {error}");
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
        let options = window_options("pain", bounds, gpui::size(gpui::px(480.), gpui::px(320.)), cx);
        let holder_for_view = Arc::clone(&pty_holder);
        let view = open_window(options, cx, |window: &mut Window, cx| {
            let terminal = cx.new(|cx| TerminalView::new(Arc::clone(&session), window, cx));
            cx.new(|_| AppView {
                terminal,
                pty: holder_for_view,
            })
        })
        .expect("open terminal window");

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
        let _ = w.update(cx, |app, _, cx| {
            app.terminal
                .update(cx, |terminal, cx| terminal.adjust_font(FONT_SIZE_STEP, cx));
        });
    });
    let w = window.clone();
    cx.on_action(move |_: &DecreaseFont, cx| {
        let _ = w.update(cx, |app, _, cx| {
            app.terminal
                .update(cx, |terminal, cx| terminal.adjust_font(-FONT_SIZE_STEP, cx));
        });
    });
    let w = window.clone();
    cx.on_action(move |_: &ResetFont, cx| {
        let default_size = cx.theme().type_.size_mono;
        let _ = w.update(cx, |app, _, cx| {
            app.terminal
                .update(cx, |terminal, cx| terminal.set_font_size(default_size, cx));
        });
    });
    let w = window.clone();
    cx.on_action(move |_: &CopySelection, cx| {
        let _ = w.update(cx, |app, _, cx| {
            match app.terminal.read(cx).session().copy_selection() {
                Ok(text) if !text.is_empty() => {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                }
                Ok(_) => {}
                Err(error) => eprintln!("pain: copy failed: {error}"),
            }
        });
    });
    let w = window.clone();
    cx.on_action(move |_: &PasteClipboard, cx| {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        let _ = w.update(cx, |app, _, cx| {
            if let Err(error) = app.terminal.read(cx).session().paste(&text) {
                eprintln!("pain: paste failed: {error}");
            }
        });
    });
    let w = window.clone();
    cx.on_action(move |_: &SelectAllTerminal, cx| {
        let _ = w.update(cx, |app, _, cx| {
            if let Err(error) = app.terminal.read(cx).session().select_all() {
                eprintln!("pain: select all failed: {error}");
            }
        });
    });
}

/// Root view: Silvestris draws the frame; the terminal is the domain child.
struct AppView {
    terminal: Entity<TerminalView>,
    #[allow(dead_code)]
    pty: Arc<Mutex<Option<Box<dyn PlatformPty>>>>,
}

impl Render for AppView {
    fn render(&mut self, _window: &mut Window, _cx: &mut gpui::Context<Self>) -> impl IntoElement {
        WindowFrame::new("pain").child(self.terminal.clone())
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
