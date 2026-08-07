//! Shared presentation helpers for local filesystem paths.

use std::borrow::Cow;
use std::path::Path;

pub(super) fn home_relative(path: &Path) -> Cow<'_, str> {
    let Some(home) = dirs::home_dir() else {
        return path.to_string_lossy();
    };
    let Ok(relative) = path.strip_prefix(home) else {
        return path.to_string_lossy();
    };
    if relative.as_os_str().is_empty() {
        Cow::Borrowed("~")
    } else {
        Cow::Owned(format!("~/{}", relative.to_string_lossy()))
    }
}
