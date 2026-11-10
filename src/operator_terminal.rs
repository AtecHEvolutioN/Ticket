//! Cancellation-safe input from the operator's local controlling terminal.
//! Tokio's standard stdin adapter uses a blocking thread that can keep runtime
//! shutdown waiting for input. A separate nonblocking TTY descriptor avoids that
//! thread without changing stdin or its parent shell's shared file flags.
use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};

use anyhow::Result;
use tokio::io::{AsyncRead, ReadBuf};

#[cfg(unix)]
pub struct OperatorTerminal {
    inner: tokio::io::unix::AsyncFd<std::fs::File>,
}

#[cfg(not(unix))]
pub struct OperatorTerminal {
    _private: (),
}

impl OperatorTerminal {
    /// Open the local controlling terminal inside an I/O-enabled Tokio runtime.
    /// No reader thread is created, so dropping the input cancels pending reads.
    #[cfg(unix)]
    pub fn open() -> Result<Self> {
        Self::open_terminal(std::path::Path::new("/dev/tty"))
    }

    #[cfg(not(unix))]
    pub fn open() -> Result<Self> {
        anyhow::bail!("operator mode currently requires a Unix controlling terminal")
    }

    #[cfg(unix)]
    fn open_terminal(path: &std::path::Path) -> Result<Self> {
        use std::{io::IsTerminal, os::unix::fs::OpenOptionsExt};

        use anyhow::Context;

        tokio::runtime::Handle::try_current()
            .context("operator terminal requires an async runtime")?;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC | libc::O_NOCTTY)
            .open(path)
            .context("open local controlling terminal")?;
        anyhow::ensure!(file.is_terminal(), "operator input is not a terminal");
        // SAFETY: File owns a valid descriptor. It is moved into the private
        // AsyncFd field, never replaced or closed separately, and therefore
        // keeps the same descriptor/open file description for its full lifetime.
        let inner = unsafe {
            tokio::io::unix::AsyncFd::register_with_interest(file, tokio::io::Interest::READABLE)
        }
        .context("register operator terminal readiness")?;
        Ok(Self { inner })
    }
}

#[cfg(unix)]
impl AsyncRead for OperatorTerminal {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        use std::{io::Read, task::ready};

        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        loop {
            let mut ready = ready!(self.inner.poll_read_ready(cx))?;
            match ready.try_io(|inner| inner.get_ref().read(buf.initialize_unfilled())) {
                Ok(Ok(length)) => {
                    buf.advance(length);
                    return Poll::Ready(Ok(()));
                }
                Ok(Err(error)) if error.kind() == io::ErrorKind::Interrupted => continue,
                Ok(Err(error)) => return Poll::Ready(Err(error)),
                Err(_would_block) => continue,
            }
        }
    }
}

#[cfg(not(unix))]
impl AsyncRead for OperatorTerminal {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Poll::Ready(Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "operator mode currently requires a Unix controlling terminal",
        )))
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::{
        fs::File,
        io::Write,
        os::fd::{AsRawFd, FromRawFd},
        path::PathBuf,
        sync::mpsc,
        time::{Duration, Instant},
    };

    use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};

    use super::OperatorTerminal;

    struct Pty {
        master: File,
        slave: File,
        slave_path: PathBuf,
    }

    impl Pty {
        fn new() -> Self {
            let mut master = -1;
            let mut slave = -1;
            // SAFETY: both output pointers are valid; null name/termios/winsize
            // request the platform defaults. Successful descriptors are owned
            // exactly once by the Files constructed immediately below.
            let result = unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null(),
                )
            };
            assert_eq!(result, 0, "openpty: {}", std::io::Error::last_os_error());
            // SAFETY: successful openpty returned new descriptors owned here.
            let master = unsafe { File::from_raw_fd(master) };
            // SAFETY: the slave is distinct from the master and newly owned.
            let slave = unsafe { File::from_raw_fd(slave) };
            let mut path = [0 as libc::c_char; 1024];
            // SAFETY: the buffer is valid for the supplied length; slave stays
            // open while ttyname_r copies its null-terminated name.
            let result =
                unsafe { libc::ttyname_r(slave.as_raw_fd(), path.as_mut_ptr(), path.len()) };
            assert_eq!(result, 0);
            // SAFETY: successful ttyname_r produced a null-terminated string in
            // the still-live buffer. PTY device paths contain valid UTF-8 here.
            let path = unsafe { std::ffi::CStr::from_ptr(path.as_ptr()) };
            Self {
                master,
                slave,
                slave_path: PathBuf::from(path.to_str().unwrap()),
            }
        }

        fn flags(&self) -> i32 {
            // SAFETY: F_GETFL reads flags from the valid, owned slave descriptor.
            let flags = unsafe { libc::fcntl(self.slave.as_raw_fd(), libc::F_GETFL) };
            assert!(flags >= 0);
            flags
        }
    }

    #[tokio::test]
    async fn reads_terminal_lines_without_changing_the_parent_descriptors_flags() {
        let mut pty = Pty::new();
        let original = pty.flags();
        assert_eq!(original & libc::O_NONBLOCK, 0);
        let terminal = OperatorTerminal::open_terminal(&pty.slave_path).unwrap();
        // SAFETY: F_GETFL/F_GETFD only read the adapter's valid owned descriptor.
        let flags = unsafe { libc::fcntl(terminal.inner.get_ref().as_raw_fd(), libc::F_GETFL) };
        // SAFETY: as above; F_GETFD checks close-on-exec ownership isolation.
        let descriptor_flags =
            unsafe { libc::fcntl(terminal.inner.get_ref().as_raw_fd(), libc::F_GETFD) };
        assert_ne!(flags & libc::O_NONBLOCK, 0);
        assert_ne!(descriptor_flags & libc::FD_CLOEXEC, 0);
        assert_eq!(pty.flags(), original);

        let mut input = BufReader::new(terminal);
        let mut line = String::new();
        assert!(
            tokio::time::timeout(Duration::from_millis(20), input.read_line(&mut line))
                .await
                .is_err()
        );
        pty.master.write_all(b"status\n").unwrap();
        tokio::time::timeout(Duration::from_secs(1), input.read_line(&mut line))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(line, "status\n");
        drop(input);
        assert_eq!(pty.flags(), original);
    }

    #[test]
    fn cancelled_read_and_runtime_shutdown_need_no_terminal_input() {
        let pty = Pty::new();
        let path = pty.slave_path.clone();
        let original = pty.flags();
        let (finished, receiver) = mpsc::channel();
        // A watchdog outside the runtime catches the exact regression: runtime
        // drop waiting forever for a blocking stdin reader without a newline.
        let worker = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let started = Instant::now();
            runtime.block_on(async {
                let mut terminal = OperatorTerminal::open_terminal(&path).unwrap();
                let mut byte = [0_u8];
                assert!(
                    tokio::time::timeout(Duration::from_millis(25), terminal.read(&mut byte))
                        .await
                        .is_err()
                );
            });
            drop(runtime);
            finished.send(started.elapsed()).unwrap();
        });
        let elapsed = receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("runtime shutdown waited for terminal input");
        assert!(elapsed < Duration::from_secs(1));
        worker.join().unwrap();
        assert_eq!(pty.flags(), original);
    }

    #[tokio::test]
    async fn ordinary_files_cannot_be_used_as_operator_terminal_input() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let error = OperatorTerminal::open_terminal(file.path()).err().unwrap();
        assert_eq!(error.to_string(), "operator input is not a terminal");
    }
}
