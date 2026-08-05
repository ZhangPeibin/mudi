use super::*;

#[test]
fn every_theme_preserves_semantic_contrast() {
    for theme_name in ThemeName::ALL {
        let theme = theme_name.palette();
        for (role, foreground, background, minimum) in [
            ("foreground", theme.foreground, theme.background, 4.5),
            ("muted", theme.muted, theme.background, 3.0),
            (
                "selected",
                theme.selected_foreground,
                theme.selected_background,
                4.5,
            ),
            ("critical", theme.critical, theme.background, 4.5),
            ("warning", theme.warning, theme.background, 4.5),
            (
                "context-normal",
                theme.context_normal,
                theme.background,
                4.5,
            ),
            (
                "context-warning",
                theme.context_warning,
                theme.background,
                4.5,
            ),
            (
                "context-critical",
                theme.context_critical,
                theme.background,
                4.5,
            ),
            ("idle", theme.idle, theme.background, 4.5),
            ("busy", theme.busy, theme.background, 4.5),
        ] {
            let ratio = contrast_ratio(foreground, background);
            assert!(
                ratio >= minimum,
                "theme {} role {role} contrast {ratio:.2} is below {minimum:.1}",
                theme_name.as_str(),
            );
        }
        let context_colors = [
            theme.context_normal,
            theme.context_warning,
            theme.context_critical,
        ];
        for pair in context_colors.windows(2) {
            assert_ne!(
                pair[0],
                pair[1],
                "theme {} merges adjacent Current Context roles",
                theme_name.as_str(),
            );
        }
        for context_color in context_colors {
            assert_ne!(
                context_color,
                theme.idle,
                "theme {} reuses Idle for Current Context",
                theme_name.as_str(),
            );
            assert_ne!(
                context_color,
                theme.busy,
                "theme {} reuses Busy for Current Context",
                theme_name.as_str(),
            );
        }
    }
}

#[test]
fn legacy_default_theme_name_loads_as_sessionmesh() {
    assert_eq!(
        ThemeName::from_name("tokenplay"),
        Some(ThemeName::SessionMesh)
    );
    assert_eq!(
        serde_json::from_str::<ThemeName>(r#""tokenplay""#).unwrap(),
        ThemeName::SessionMesh
    );
    assert_eq!(
        serde_json::to_string(&ThemeName::SessionMesh).unwrap(),
        r#""sessionmesh""#
    );
}

fn contrast_ratio(foreground: Color, background: Color) -> f64 {
    let foreground = luminance(foreground);
    let background = luminance(background);
    let (lighter, darker) = if foreground > background {
        (foreground, background)
    } else {
        (background, foreground)
    };
    (lighter + 0.05) / (darker + 0.05)
}

fn luminance(color: Color) -> f64 {
    let Color::Rgb(red, green, blue) = color else {
        panic!("built-in Themes must use RGB colors");
    };
    0.2126 * linear(red) + 0.7152 * linear(green) + 0.0722 * linear(blue)
}

fn linear(channel: u8) -> f64 {
    let value = f64::from(channel) / 255.0;
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}
