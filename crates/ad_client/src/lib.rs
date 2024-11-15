//! A simple 9p based client for interacting with ad
#![warn(
    clippy::complexity,
    clippy::correctness,
    clippy::style,
    future_incompatible,
    missing_debug_implementations,
    missing_docs,
    rust_2018_idioms,
    rustdoc::all,
    clippy::undocumented_unsafe_blocks
)]
use ninep::client::{ReadLineIter, UnixClient};
use std::{io, io::Write, os::unix::net::UnixStream};

mod event;

pub use ad_event::Source;
pub use event::{EventFilter, Outcome};

/// A simple 9p client for ad
#[derive(Debug, Clone)]
pub struct Client {
    inner: UnixClient,
}

impl Client {
    /// Create a new client connected to `ad` over its 9p unix socket
    pub fn new() -> io::Result<Self> {
        Ok(Self {
            inner: UnixClient::new_unix("ad", "")?,
        })
    }

    pub(crate) fn event_lines(&mut self, buffer: &str) -> io::Result<ReadLineIter<UnixStream>> {
        self.inner.iter_lines(format!("buffers/{buffer}/event"))
    }

    pub(crate) fn write_event(&mut self, buffer: &str, event_line: &str) -> io::Result<()> {
        self.inner
            .write_str(format!("buffers/{buffer}/event"), 0, event_line)?;
        Ok(())
    }

    /// Get the currently active buffer id.
    pub fn current_buffer(&mut self) -> io::Result<String> {
        self.inner.read_str("buffers/current")
    }

    fn _read_buffer_file(&mut self, buffer: &str, file: &str) -> io::Result<String> {
        self.inner.read_str(format!("buffers/{buffer}/{file}"))
    }

    /// Read the contents of the dot of the given buffer
    pub fn read_dot(&mut self, buffer: &str) -> io::Result<String> {
        self._read_buffer_file(buffer, "dot")
    }

    /// Read the body of the given buffer.
    pub fn read_body(&mut self, buffer: &str) -> io::Result<String> {
        self._read_buffer_file(buffer, "body")
    }

    /// Read the current dot address of the given buffer.
    pub fn read_addr(&mut self, buffer: &str) -> io::Result<String> {
        self._read_buffer_file(buffer, "addr")
    }

    /// Read the filename of the given buffer
    pub fn read_filename(&mut self, buffer: &str) -> io::Result<String> {
        self._read_buffer_file(buffer, "filename")
    }

    /// Read the x-address of the given buffer.
    ///
    /// This is only used by the filesystem interface of `ad` and will not affect the current
    /// editor state.
    pub fn read_xaddr(&mut self, buffer: &str) -> io::Result<String> {
        self._read_buffer_file(buffer, "xaddr")
    }

    /// Read the x-dot of the given buffer.
    ///
    /// This is only used by the filesystem interface of `ad` and will not affect the current
    /// editor state.
    pub fn read_xdot(&mut self, buffer: &str) -> io::Result<String> {
        self._read_buffer_file(buffer, "xdot")
    }

    fn _write_buffer_file(
        &mut self,
        buffer: &str,
        file: &str,
        offset: u64,
        content: &[u8],
    ) -> io::Result<usize> {
        self.inner
            .write(format!("buffers/{buffer}/{file}"), offset, content)
    }

    /// Replace the dot of the given buffer with the provided string.
    pub fn write_dot(&mut self, buffer: &str, content: &str) -> io::Result<usize> {
        self._write_buffer_file(buffer, "dot", 0, content.as_bytes())
    }

    /// Append the provided string to the given buffer.
    pub fn append_to_body(&mut self, buffer: &str, content: &str) -> io::Result<usize> {
        self._write_buffer_file(buffer, "body", 0, content.as_bytes())
    }

    /// Set the addr of the given buffer.
    pub fn write_addr(&mut self, buffer: &str, addr: &str) -> io::Result<usize> {
        self._write_buffer_file(buffer, "addr", 0, addr.as_bytes())
    }

    /// Replace the xdot of the given buffer with the provided string.
    pub fn write_xdot(&mut self, buffer: &str, content: &str) -> io::Result<usize> {
        self._write_buffer_file(buffer, "xdot", 0, content.as_bytes())
    }

    /// Set the xaddr of the given buffer.
    pub fn write_xaddr(&mut self, buffer: &str, content: &str) -> io::Result<usize> {
        self._write_buffer_file(buffer, "xaddr", 0, content.as_bytes())
    }

    /// Send a control message to ad.
    pub fn ctl(&mut self, command: &str, args: &str) -> io::Result<()> {
        self.inner
            .write("ctl", 0, format!("{command} {args}").as_bytes())?;

        Ok(())
    }

    /// Echo a string message in the status line.
    pub fn echo(&mut self, msg: impl AsRef<str>) -> io::Result<()> {
        self.ctl("echo", msg.as_ref())
    }

    /// Open the requested file.
    pub fn open(&mut self, path: impl AsRef<str>) -> io::Result<()> {
        self.ctl("open", path.as_ref())
    }

    /// Open the requested file in a new window.
    pub fn open_in_new_window(&mut self, path: impl AsRef<str>) -> io::Result<()> {
        self.ctl("open-in-new-window", path.as_ref())
    }

    /// Reload the currently active buffer.
    pub fn reload_current_buffer(&mut self) -> io::Result<()> {
        self.ctl("reload", "")
    }

    /// Run a provided [EventFilter] until it exits or errors
    pub fn run_event_filter<F>(&mut self, buffer: &str, filter: F) -> io::Result<()>
    where
        F: EventFilter,
    {
        event::run_filter(buffer, filter, self)
    }

    /// Create a [Write] impl that can be used to continuously write to the given path
    pub fn body_writer(&self, bufid: &str) -> io::Result<impl Write> {
        let client = UnixClient::new_unix("ad", "")?;

        Ok(BodyWriter {
            path: format!("buffers/{bufid}/body"),
            client,
        })
    }
}

/// A writer for appending to the body of a buffer
#[derive(Debug)]
pub struct BodyWriter {
    path: String,
    client: UnixClient,
}

impl Write for BodyWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.client.write(&self.path, 0, buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
