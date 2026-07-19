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
    Capture(Arc<Mutex<Vec<u8>>>),
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
        (Self(SinkInner::Capture(buf.clone())), CaptureHandle { buf })
    }
}

impl Write for Sink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match &self.0 {
            SinkInner::Stdout => io::stdout().lock().write(buf),
            #[cfg(test)]
            SinkInner::Capture(shared) => {
                shared.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }
        }
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        match &self.0 {
            SinkInner::Stdout => io::stdout().lock().write_all(buf),
            #[cfg(test)]
            SinkInner::Capture(shared) => {
                shared.lock().unwrap().extend_from_slice(buf);
                Ok(())
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match &self.0 {
            SinkInner::Stdout => io::stdout().flush(),
            #[cfg(test)]
            SinkInner::Capture(_) => Ok(()),
        }
    }
}

/// Reads back everything a capture [`Sink`] has been fed.
#[cfg(test)]
pub(super) struct CaptureHandle {
    buf: Arc<Mutex<Vec<u8>>>,
}

#[cfg(test)]
impl CaptureHandle {
    pub(super) fn bytes(&self) -> Vec<u8> {
        self.buf.lock().unwrap().clone()
    }
}
