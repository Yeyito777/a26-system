//! A nonblocking newline-delimited socket and an eventfd wake-up, shared by
//! the Exocortex app's daemon worker and the `exocortex-notify` bridge.
//! Neither ever blocks on I/O: both sleep in `poll(2)` on these fds.
#![allow(dead_code)]

use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

/// One line longer than this is a broken peer, not data: the connection is
/// dropped instead of growing without bound.
const MAX_LINE: usize = 64 * 1024 * 1024;

pub struct LineSocket {
    stream: UnixStream,
    read: Vec<u8>,
    /// Where the next search for a newline starts in `read`.
    scanned: usize,
    write: Vec<u8>,
    written: usize,
}

impl LineSocket {
    pub fn connect(path: &Path) -> io::Result<Self> {
        let stream = UnixStream::connect(path)?;
        stream.set_nonblocking(true)?;
        Ok(Self {
            stream,
            read: Vec::new(),
            scanned: 0,
            write: Vec::new(),
            written: 0,
        })
    }

    pub fn fd(&self) -> RawFd {
        self.stream.as_raw_fd()
    }

    /// Poll events to wait for: readable, and writable while output waits.
    pub fn interest(&self) -> i16 {
        if self.wants_write() {
            libc::POLLIN | libc::POLLOUT
        } else {
            libc::POLLIN
        }
    }

    pub fn wants_write(&self) -> bool {
        self.written < self.write.len()
    }

    /// Queues one line (a newline is added) and writes what the socket takes.
    pub fn send(&mut self, line: &str) -> io::Result<()> {
        self.write.extend_from_slice(line.as_bytes());
        self.write.push(b'\n');
        self.flush()
    }

    /// Writes queued output until the socket would block.
    pub fn flush(&mut self) -> io::Result<()> {
        while self.written < self.write.len() {
            match self.stream.write(&self.write[self.written..]) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(count) => self.written += count,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
        if self.written == self.write.len() {
            self.write.clear();
            self.written = 0;
        }
        Ok(())
    }

    /// Reads what is available and hands each complete line to `line`.
    /// Returns an error at end of stream or on a socket error.
    pub fn read_lines(&mut self, mut line: impl FnMut(&str)) -> io::Result<()> {
        let mut chunk = [0_u8; 64 * 1024];
        loop {
            match self.stream.read(&mut chunk) {
                Ok(0) => {
                    self.take_lines(&mut line);
                    return Err(io::ErrorKind::UnexpectedEof.into());
                }
                Ok(count) => {
                    self.read.extend_from_slice(&chunk[..count]);
                    self.take_lines(&mut line);
                    if self.read.len() > MAX_LINE {
                        return Err(io::Error::new(io::ErrorKind::InvalidData, "line too long"));
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
    }

    fn take_lines(&mut self, line: &mut impl FnMut(&str)) {
        let mut start = 0;
        let mut search = self.scanned;
        while let Some(offset) = self.read[search..].iter().position(|b| *b == b'\n') {
            let end = search + offset;
            if let Ok(text) = std::str::from_utf8(&self.read[start..end]) {
                let text = text.trim();
                if !text.is_empty() {
                    line(text);
                }
            }
            start = end + 1;
            search = start;
        }
        self.read.drain(..start);
        self.scanned = self.read.len();
    }
}

/// An eventfd: one side wakes, the other polls it and drains.
pub struct Waker {
    fd: OwnedFd,
}

impl Waker {
    pub fn new() -> io::Result<Self> {
        // SAFETY: eventfd takes scalar flags and returns a new fd or -1.
        let fd = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: a freshly created descriptor this value now owns.
        Ok(Self {
            fd: unsafe { OwnedFd::from_raw_fd(fd) },
        })
    }

    pub fn fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }

    pub fn wake(&self) {
        let one: u64 = 1;
        // SAFETY: writes 8 bytes from a live u64 to our eventfd.
        unsafe {
            libc::write(self.fd.as_raw_fd(), (&one as *const u64).cast(), 8);
        }
    }

    pub fn drain(&self) {
        let mut value: u64 = 0;
        // SAFETY: reads 8 bytes into a live u64 from our nonblocking eventfd.
        unsafe {
            libc::read(self.fd.as_raw_fd(), (&mut value as *mut u64).cast(), 8);
        }
    }
}

/// `poll(2)` over `fds` for at most `timeout` (`None`: until something
/// happens). Interrupted waits return normally.
pub fn poll(fds: &mut [libc::pollfd], timeout: Option<Duration>) -> io::Result<()> {
    let milliseconds = timeout.map_or(-1, |t| {
        // Round up so a 0.4 ms deadline does not spin at zero.
        let ms = t.as_micros().div_ceil(1000);
        ms.min(i32::MAX as u128) as i32
    });
    // SAFETY: a valid slice of initialized pollfds for the duration of the call.
    let result = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, milliseconds) };
    if result < 0 {
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
    Ok(())
}

pub fn pollfd(fd: RawFd, events: i16) -> libc::pollfd {
    libc::pollfd {
        fd,
        events,
        revents: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    #[test]
    fn lines_split_across_reads_and_writes_queue() {
        let path = std::env::temp_dir().join(format!("exo-line-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        let mut client = LineSocket::connect(&path).unwrap();
        let (mut server, _) = listener.accept().unwrap();
        client.send(r#"{"type":"ping"}"#).unwrap();
        let mut received = [0_u8; 16];
        server.read_exact(&mut received).unwrap();
        assert_eq!(&received, b"{\"type\":\"ping\"}\n");

        server.write_all(b"{\"a\":1}\n{\"b\"").unwrap();
        let mut lines = Vec::new();
        let mut fds = [pollfd(client.fd(), client.interest())];
        poll(&mut fds, Some(Duration::from_secs(1))).unwrap();
        client
            .read_lines(|line| lines.push(line.to_owned()))
            .unwrap();
        assert_eq!(lines, vec![r#"{"a":1}"#]);
        server.write_all(b":2}\n\n").unwrap();
        poll(&mut fds, Some(Duration::from_secs(1))).unwrap();
        client
            .read_lines(|line| lines.push(line.to_owned()))
            .unwrap();
        assert_eq!(lines, vec![r#"{"a":1}"#, r#"{"b":2}"#]);
        drop(server);
        poll(&mut fds, Some(Duration::from_secs(1))).unwrap();
        assert!(client.read_lines(|_| {}).is_err());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_waker_wakes_a_poll() {
        let waker = Waker::new().unwrap();
        let mut fds = [pollfd(waker.fd(), libc::POLLIN)];
        poll(&mut fds, Some(Duration::ZERO)).unwrap();
        assert_eq!(fds[0].revents, 0);
        waker.wake();
        poll(&mut fds, Some(Duration::from_secs(1))).unwrap();
        assert_ne!(fds[0].revents & libc::POLLIN, 0);
        waker.drain();
        fds[0].revents = 0;
        poll(&mut fds, Some(Duration::ZERO)).unwrap();
        assert_eq!(fds[0].revents, 0);
    }
}
