//! UI 文本宽度与截断工具。
//!
//! 终端列宽不等于 Unicode 字符数量。本模块集中处理标题对齐和长文本缩写，
//! 让各功能模块只声明目标宽度，不重复实现字符宽度算法。

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// 从字符串开头保留内容，超长时在末尾加省略号。
pub(super) fn shorten_start(value: &str, width: usize) -> String {
    shorten(value, width, false)
}

/// 从字符串末尾保留内容，超长时在开头加省略号。
pub(super) fn shorten_end(value: &str, width: usize) -> String {
    shorten(value, width, true)
}

/// 按字符数量截断字符串，可选择保留头部或尾部。
fn shorten(value: &str, width: usize, keep_end: bool) -> String {
    let characters = value.chars().collect::<Vec<_>>();
    if characters.len() <= width {
        return value.into();
    }
    if width <= 3 {
        return ".".repeat(width);
    }
    let kept = width - 3;
    if keep_end {
        format!(
            "...{}",
            characters[characters.len() - kept..]
                .iter()
                .collect::<String>()
        )
    } else {
        format!("{}...", characters[..kept].iter().collect::<String>())
    }
}

/// 把标题精确填充或截断到指定显示宽度，保证后续列不发生位移。
pub(super) fn fit(value: &str, width: usize, keep_end: bool) -> String {
    let value_width = UnicodeWidthStr::width(value);
    if value_width <= width {
        return format!("{value}{}", " ".repeat(width - value_width));
    }
    if width <= 3 {
        return ".".repeat(width);
    }

    let available = width - 3;
    let mut kept = Vec::new();
    let mut kept_width = 0;
    let characters = if keep_end {
        value.chars().rev().collect::<Vec<_>>()
    } else {
        value.chars().collect::<Vec<_>>()
    };
    for character in characters {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if kept_width + character_width > available {
            break;
        }
        kept.push(character);
        kept_width += character_width;
    }
    if keep_end {
        kept.reverse();
    }
    let kept = kept.into_iter().collect::<String>();
    let fitted = if keep_end {
        format!("...{kept}")
    } else {
        format!("{kept}...")
    };
    let fitted_width = UnicodeWidthStr::width(fitted.as_str());
    format!("{fitted}{}", " ".repeat(width.saturating_sub(fitted_width)))
}

/// 把值右对齐到固定显示宽度；超长时复用 Unicode 安全的截断规则。
pub(super) fn fit_right(value: &str, width: usize, keep_end: bool) -> String {
    let value_width = UnicodeWidthStr::width(value);
    if value_width <= width {
        return format!("{}{value}", " ".repeat(width - value_width));
    }
    fit(value, width, keep_end)
}

/// 返回输入框能容纳的 Unicode 尾部，供编辑长标题时跟随光标。
pub(super) fn visible_input_tail(value: &str, width: usize) -> String {
    let mut visible = Vec::new();
    let mut visible_width = 0;
    for character in value.chars().rev() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if visible_width + character_width > width {
            break;
        }
        visible.push(character);
        visible_width += character_width;
    }
    visible.reverse();
    visible.into_iter().collect()
}

#[cfg(test)]
#[path = "text_test.rs"]
mod tests;
