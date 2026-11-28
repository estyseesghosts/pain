//! Canned VT fixtures for Pain's fixture and stress application modes.
//!
//! These are application policy, not terminal semantics: the reusable
//! `pain-terminal-core` crate does not inherit fixture data. Bytes are fed
//! through the public `TerminalSession` API exactly as a PTY would feed them.

use pain_terminal_core::TerminalSession;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fixture {
    /// ANSI 16-color, bold, and cursor placement.
    Ansi,
    /// 256-color and 24-bit color cells.
    Colors,
    /// Wide graphemes, underline, bold/italic, and a moved cursor.
    WideAndStyles,
    /// A row selection installed through Ghostty's selection option.
    Selection,
}

impl Fixture {
    pub const ALL: [Fixture; 4] = [
        Fixture::Ansi,
        Fixture::Colors,
        Fixture::WideAndStyles,
        Fixture::Selection,
    ];

    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Fixture::Ansi => "ansi",
            Fixture::Colors => "colors",
            Fixture::WideAndStyles => "wide-and-styles",
            Fixture::Selection => "selection",
        }
    }

    #[must_use]
    pub fn bytes(self) -> &'static [u8] {
        match self {
            Fixture::Ansi => b"\x1b[2J\x1b[HHi \x1b[1;32mGreen\x1b[0m\r\n\x1b[31mRed\x1b[0m and plain\x1b[3;5H",
            Fixture::Colors => b"\x1b[2J\x1b[H\x1b[38;5;196m256\x1b[0m \x1b[48;2;12;34;56m\x1b[38;2;255;16;32mTrue\x1b[0m",
            Fixture::WideAndStyles => b"\x1b[2J\x1b[H\x1b[4mUnder\x1b[0m \x1b[1mBold\x1b[0m\r\n\x1b[1;35mG\x1b[0mhostty \xE7\x95\x8C\xE7\x95\x8C\r\n\x1b[6;10H",
            Fixture::Selection => b"\x1b[2J\x1b[HSelected text here\r\nsecond row",
        }
    }

    /// Feed the fixture through a session, preserving Ghostty-owned selection
    /// state where the fixture asks for it.
    pub fn apply_session(self, session: &TerminalSession) {
        session.write_vt(self.bytes());
        if self == Fixture::Selection {
            session
                .set_row_selection(0, 0, 7)
                .expect("selection fixture installs a Ghostty selection");
        }
    }

    /// A short deterministic chunk stream used by the stress application mode.
    /// Each chunk is a valid, independently writable piece of VT.
    #[must_use]
    pub fn chunks(self) -> Vec<&'static [u8]> {
        match self {
            Fixture::Ansi => vec![
                b"\x1b[2J\x1b[H",
                b"chunk ",
                b"\x1b[1;32mone\x1b[0m\r\n",
                b"chunk two",
            ],
            Fixture::Colors => vec![
                b"\x1b[2J\x1b[H",
                b"\x1b[38;5;196mred\x1b[0m ",
                b"\x1b[38;2;1;2;3mrgb\x1b[0m",
            ],
            Fixture::WideAndStyles => vec![
                b"\x1b[2J\x1b[H",
                b"\xE7\x95\x8C\xE7\x95\x8C",
                b" \x1b[4munder\x1b[0m",
            ],
            Fixture::Selection => vec![b"\x1b[2J\x1b[H", b"selected stream"],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pain_terminal_core::{NoopWakeup, RowSelection};

    fn session() -> TerminalSession {
        TerminalSession::new(80, 24, Box::new(NoopWakeup)).expect("session")
    }

    #[test]
    fn every_fixture_renders_a_snapshot() {
        for fixture in Fixture::ALL {
            let session = session();
            fixture.apply_session(&session);
            let snapshot = session.render_snapshot().expect("snapshot");
            assert_eq!(
                (snapshot.cols, snapshot.meta.rows),
                (80, 24),
                "{}",
                fixture.name()
            );
            assert!(
                snapshot.line(0).iter().any(|c| !c.text.is_empty()),
                "{}",
                fixture.name()
            );
        }
    }

    #[test]
    fn selection_fixture_reaches_render_state() {
        let session = session();
        Fixture::Selection.apply_session(&session);
        let snapshot = session.render_snapshot().expect("snapshot");
        assert_eq!(
            snapshot.meta.selections[0],
            Some(RowSelection {
                start_x: 0,
                end_x: 7
            })
        );
        assert!(snapshot.line(0)[3].selected);
        assert!(!snapshot.line(0)[9].selected);
    }

    #[test]
    fn chunks_are_non_empty() {
        for fixture in Fixture::ALL {
            assert!(fixture.chunks().iter().all(|c| !c.is_empty()));
        }
    }
}
