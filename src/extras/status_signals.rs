#[cfg(unix)]
use std::io::Write;
#[cfg(unix)]
use std::os::unix::net::UnixStream;

#[derive(Clone)]
pub struct StatusSignals {
    path: String,
}

impl StatusSignals {
    #[allow(dead_code)]
    pub fn new(path: String) -> Self {
        Self { path }
    }

    #[cfg(unix)]
    pub fn send_start(&self) {
        let _ = (|| -> std::io::Result<()> {
            let mut stream = UnixStream::connect(&self.path)?;
            stream.write_all(b"start\n")?;
            Ok(())
        })();
    }

    #[cfg(not(unix))]
    pub fn send_start(&self) {}

    #[cfg(unix)]
    pub fn send_stop(&self) {
        let _ = (|| -> std::io::Result<()> {
            let mut stream = UnixStream::connect(&self.path)?;
            stream.write_all(b"stop\n")?;
            Ok(())
        })();
    }

    #[cfg(not(unix))]
    pub fn send_stop(&self) {}

    #[cfg(unix)]
    #[allow(dead_code)]
    pub fn send_git_conflict(&self) {
        let _ = (|| -> std::io::Result<()> {
            let mut stream = UnixStream::connect(&self.path)?;
            stream.write_all(b"git-conflict\n")?;
            Ok(())
        })();
    }

    #[cfg(not(unix))]
    #[allow(dead_code)]
    pub fn send_git_conflict(&self) {}
}
