//! Multi-instance GPUI regression: two terminal views must coexist with
//! independent focus and session state, with no global focus authority.

#![cfg(feature = "gpui-tests")]

use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use gpui::{
    div, prelude::*, px, size, AnyWindowHandle, App, AppContext, Context, Entity,
    HeadlessAppContext, NoopTextSystem, Render, Window,
};
use pain::gpui::render::{IncreaseFont, TerminalFont, TerminalView};
use pain::terminal::session::{NoopWakeup, TerminalSession};

fn serial() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn run_render_test(body: impl FnOnce() + Send + 'static) {
    let handle = std::thread::Builder::new()
        .name("gpui-two-views-test".into())
        .stack_size(32 * 1024 * 1024)
        .spawn(move || {
            let _guard = serial();
            body();
        })
        .expect("spawn GPUI two-view test thread");
    if let Err(payload) = handle.join() {
        std::panic::resume_unwind(payload);
    }
}

struct TwoPanes {
    first: Entity<TerminalView>,
    second: Option<Entity<TerminalView>>,
}

impl Render for TwoPanes {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let row = div().size_full().child(self.first.clone());
        match &self.second {
            Some(second) => row.child(second.clone()),
            None => row,
        }
    }
}

fn make_session() -> Arc<TerminalSession> {
    Arc::new(TerminalSession::new(80, 24, Box::new(NoopWakeup)).expect("session"))
}

#[test]
fn two_views_keep_independent_focus_and_session_state() {
    run_render_test(|| {
        let mut app = HeadlessAppContext::new(Arc::new(NoopTextSystem::new()));

        let first_session = make_session();
        let second_session = Arc::clone(&first_session);
        let handle = app
            .open_window(size(px(800.0), px(400.0)), move |window, cx| {
                let first = cx.new(|cx| {
                    TerminalView::new(
                        Arc::clone(&first_session),
                        TerminalFont::new("monospace", 14.0),
                        window,
                        cx,
                    )
                });
                let second = cx.new(|cx| {
                    TerminalView::new(
                        second_session,
                        TerminalFont::new("monospace", 14.0),
                        window,
                        cx,
                    )
                });
                cx.new(|_| TwoPanes {
                    first,
                    second: Some(second),
                })
            })
            .expect("open two-pane window");
        let window: AnyWindowHandle = *handle;
        app.run_until_parked();

        let (first_view, second_view) = app.update(|cx: &mut App| {
            let root = handle.root(cx).expect("root entity");
            let panes = root.read(cx);
            (
                panes.first.clone(),
                panes.second.clone().expect("second pane present"),
            )
        });
        let first_focus = app.update(|cx: &mut App| first_view.read(cx).focus());
        let second_focus = app.update(|cx: &mut App| second_view.read(cx).focus());

        // Building the second view never steals focus from the first.
        app.update_window(window, |_, window, cx| {
            window.focus(&first_focus, cx);
            window.draw(cx).clear(cx);
        })
        .expect("focus first pane");
        app.update_window(window, |_, window, _| {
            assert!(first_focus.is_focused(window));
            assert!(!second_focus.is_focused(window));
        })
        .expect("verify first pane focus");

        // A local action reaches only the focused pane's session.
        app.update_window(window, |_, window, cx| {
            window.dispatch_action(Box::new(IncreaseFont), cx);
            window.draw(cx).clear(cx);
        })
        .expect("dispatch to first pane");
        app.run_until_parked();
        let (first_size, second_size) = app.update(|cx: &mut App| {
            (
                first_view.read(cx).font_size(),
                second_view.read(cx).font_size(),
            )
        });
        assert!(first_size > 14.0, "focused pane must receive the action");
        assert_eq!(second_size, 14.0, "unfocused pane must not change");

        // Focusing the other pane routes the next action to it instead.
        app.update_window(window, |_, window, cx| {
            window.focus(&second_focus, cx);
            window.draw(cx).clear(cx);
        })
        .expect("focus second pane");
        app.update_window(window, |_, window, cx| {
            window.dispatch_action(Box::new(IncreaseFont), cx);
            window.draw(cx).clear(cx);
        })
        .expect("dispatch to second pane");
        app.run_until_parked();
        let (first_after, second_after) = app.update(|cx: &mut App| {
            (
                first_view.read(cx).font_size(),
                second_view.read(cx).font_size(),
            )
        });
        assert_eq!(first_after, first_size, "first pane keeps its own state");
        assert!(
            second_after > 14.0,
            "second pane must receive its own action"
        );

        // Destroying one view leaves the other fully usable.
        drop(second_view);
        app.update(|cx: &mut App| {
            let root = handle.root(cx).expect("root entity");
            root.update(cx, |panes, _| panes.second = None);
        });
        app.run_until_parked();
        let surviving = app.update(|cx: &mut App| first_view.read(cx).font_size());
        assert_eq!(surviving, first_after, "surviving pane keeps its state");
    });
}
