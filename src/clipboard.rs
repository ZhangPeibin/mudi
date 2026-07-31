//! Explicit macOS clipboard writes requested from Session Detail.

use std::io::{self, Write};
use std::process::{Command, Stdio};

/// System seam for writing one exact text value to the clipboard.
pub trait Clipboard {
    fn write_text(&mut self, value: &str) -> io::Result<()>;
}

/// macOS clipboard adapter backed by the system `pbcopy` command.
pub struct MacosClipboard;

impl Clipboard for MacosClipboard {
    fn write_text(&mut self, value: &str) -> io::Result<()> {
        let mut child = Command::new("/usr/bin/pbcopy")
            .stdin(Stdio::piped())
            .spawn()?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("pbcopy stdin unavailable"))?;
        let write_result = stdin.write_all(value.as_bytes());
        drop(stdin);

        let status = child.wait()?;
        write_result?;
        if status.success() {
            Ok(())
        } else {
            Err(io::Error::other(format!(
                "pbcopy exited with status {status}"
            )))
        }
    }
}
