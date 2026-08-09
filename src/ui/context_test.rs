use super::super::theme::ThemeName;
use super::*;
use crate::domain::{ContextState, SessionState};

#[test]
fn role_uses_the_configured_warning_and_critical_thresholds() {
    assert_eq!(role(WARN_PCT - 0.1), Role::Normal);
    assert_eq!(role(WARN_PCT), Role::Warning);
    assert_eq!(role(CRIT_PCT), Role::Critical);
}

#[test]
fn progress_uses_shared_thresholds_and_clamps_the_bar_without_clamping_percentage() {
    let theme = ThemeName::SessionMesh.palette();
    let cases = [
        (699, "69.9%", Role::Normal, theme.context_normal),
        (700, "70.0%", Role::Warning, theme.context_warning),
        (849, "84.9%", Role::Warning, theme.context_warning),
        (850, "85.0%", Role::Critical, theme.context_critical),
        (1_000, "100.0%", Role::Critical, theme.context_critical),
        (1_125, "112.5%", Role::Critical, theme.context_critical),
    ];

    for (tokens, percentage, expected_role, color) in cases {
        let progress = Progress::new(
            ContextState {
                tokens,
                window: Some(1_000),
            },
            SessionState::Busy,
        );
        let line = progress.line(Presentation::Compact, "Context", 11, 40, theme);
        let rendered = line.to_string();

        assert_eq!(progress.role(), expected_role);
        assert_eq!(progress.percent_text(), percentage);
        assert!(rendered.contains(percentage));
        assert_eq!(line.spans.last().unwrap().style.fg, Some(color));
        if tokens >= 1_000 {
            let bar = rendered
                .split_once('[')
                .and_then(|(_, rest)| rest.split_once(']'))
                .map(|(bar, _)| bar)
                .expect("compact progress should include a bar");
            assert!(bar.chars().all(|cell| cell == '#'));
        }
    }
}

#[test]
fn progress_uses_neutral_empty_bar_for_unknown_window_and_exited_session() {
    let theme = ThemeName::SessionMesh.palette();
    let unknown = Progress::new(
        ContextState {
            tokens: 123_456,
            window: None,
        },
        SessionState::Busy,
    );
    let unknown_line = unknown.line(Presentation::Expanded, "Occupancy", 16, 160, theme);

    assert_eq!(unknown.role(), Role::Neutral);
    assert!(unknown_line.to_string().contains("123.5k / N/A"));
    assert!(unknown_line.to_string().contains("(N/A)"));
    assert!(unknown_line
        .to_string()
        .contains("[--------------------------------]"));
    assert_eq!(
        unknown_line.spans.last().unwrap().style.fg,
        Some(theme.muted)
    );

    let exited = Progress::new(
        ContextState {
            tokens: 900,
            window: Some(1_000),
        },
        SessionState::Exited,
    );
    let exited_line = exited.line(Presentation::Compact, "Context", 11, 40, theme);

    assert_eq!(exited.role(), Role::Neutral);
    assert_eq!(exited.percent_text(), "90.0%");
    assert_eq!(
        exited_line.spans.last().unwrap().style.fg,
        Some(theme.muted)
    );
    assert_eq!(
        exited_line
            .spans
            .iter()
            .find(|span| span.content.contains('#'))
            .map(|span| span.style.fg),
        Some(Some(theme.muted))
    );
}

#[test]
fn progress_drops_the_bar_before_percentage_or_na_on_narrow_surfaces() {
    let theme = ThemeName::SessionMesh.palette();
    let known = Progress::new(
        ContextState {
            tokens: 500,
            window: Some(1_000),
        },
        SessionState::Busy,
    );
    let compact = known.line(Presentation::Compact, "Context", 11, 20, theme);
    let expanded = known.line(Presentation::Expanded, "Occupancy", 16, 80, theme);
    let unknown = Progress::new(
        ContextState {
            tokens: 500,
            window: None,
        },
        SessionState::Busy,
    )
    .line(Presentation::Compact, "Context", 11, 20, theme);

    assert!(compact.to_string().contains("50.0%"));
    assert!(!compact.to_string().contains('['));
    assert!(expanded.to_string().contains("500 / 1.0k (50.0%)"));
    assert!(!expanded.to_string().contains('['));
    assert!(unknown.to_string().contains("N/A"));
    assert!(!unknown.to_string().contains('['));
}
