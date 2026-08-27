//! Best-effort macOS user notifications.

use crate::application::alerts::NotificationPort;
use std::process::{Command, Stdio};

pub struct MacosNotification;

impl NotificationPort for MacosNotification {
    fn notify(&self, message: &str) {
        let _ = Command::new("/usr/bin/osascript")
            .args([
                "-e",
                "on run argv",
                "-e",
                "display notification (item 1 of argv) with title \"Mudi\"",
                "-e",
                "end run",
                "--",
                message,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
}
