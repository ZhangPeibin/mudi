//! Clipboard capability used by presentation commands.

/// Writes one complete text value to the system clipboard.
pub trait ClipboardPort {
    fn write_text(&mut self, value: &str) -> std::io::Result<()>;
}
