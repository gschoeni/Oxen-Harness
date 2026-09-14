//! The byte sink every live-mode writer paints through.
//!
//! Production code writes to stdout; tests swap in a capturing sink so painted
//! escape sequences can be replayed through a terminal emulator and asserted
//! on as a screen grid (see `test_support`). Cloneable so [`Live`], the
//! [`CrlfWriter`] region adapter, and the streamed-Markdown writer can all
//! share one destination.
//!
//! [`Live`]: super::Live
//! [`CrlfWriter`]: super::terminal::CrlfWriter

use std::io::{self, Write};
#[cfg(test)]
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub(super) struct Sink(SinkInner);

#[derive(Clone)]
enum SinkInner {
    Stdout,
    #[cfg(test)]
    Capture {
        buf: Arc<Mutex<Vec<u8>>>,
        /// What a cursor-position probe answers (see [`Sink::probe_cursor_row`]).
        cursor_row: Arc<Mutex<Option<u16>>>,
    },
}

impl Sink {
    pub(super) fn stdout() -> Self {
        Self(SinkInner::Stdout)
    }

    /// A sink that appends everything written into a shared buffer, paired
    /// with the handle tests read it back through.
    #[cfg(test)]
    pub(super) fn capture() -> (Self, CaptureHandle) {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let cursor_row = Arc::new(Mutex::new(None));
        (
            Self(SinkInner::Capture {
                buf: buf.clone(),
                cursor_row: cursor_row.clone(),
            }),
            CaptureHandle { buf, cursor_row },
        )
    }

    /// Where the output cursor is (1-based row): asked of the terminal for
    /// stdout (see [`super::terminal::probe_cursor_row`]), preset by the test
    /// for a capture.
    pub(super) fn probe_cursor_row(&self) -> Option<u16> {
        match &self.0 {
            SinkInner::Stdout => super::terminal::probe_cursor_row(),
            #[cfg(test)]
            SinkInner::Capture { cursor_row, .. } => *cursor_row.lock().unwrap(),
        }
    }
}

impl Write for Sink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match &self.0 {
            SinkInner::Stdout => io::stdout().lock().write(buf),
            #[cfg(test)]
            SinkInner::Capture { buf: shared, .. } => {
                shared.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }
        }
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        match &self.0 {
            SinkInner::Stdout => io::stdout().lock().write_all(buf),
            #[cfg(test)]
            SinkInner::Capture { buf: shared, .. } => {
                shared.lock().unwrap().extend_from_slice(buf);
                Ok(())
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match &self.0 {
            SinkInner::Stdout => io::stdout().flush(),
            #[cfg(test)]
            SinkInner::Capture { .. } => Ok(()),
        }
    }
}

/// Reads back everything a capture [`Sink`] has been fed.
#[cfg(test)]
pub(super) struct CaptureHandle {
    buf: Arc<Mutex<Vec<u8>>>,
    cursor_row: Arc<Mutex<Option<u16>>>,
}

#[cfg(test)]
impl CaptureHandle {
    pub(super) fn bytes(&self) -> Vec<u8> {
        self.buf.lock().unwrap().clone()
    }

    /// Append bytes as if something outside [`Live`] painted them — what a
    /// cooked-mode picker draws while the layout is suspended.
    ///
    /// [`Live`]: super::Live
    pub(super) fn inject(&self, bytes: &[u8]) {
        self.buf.lock().unwrap().extend_from_slice(bytes);
    }

    /// Preset what the next cursor-position probe reports (1-based row).
    pub(super) fn set_cursor_row(&self, row: Option<u16>) {
        *self.cursor_row.lock().unwrap() = row;
    }
}
