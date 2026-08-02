use ratatui::style::Color;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy)]
pub(super) struct Theme {
    pub(super) background: Color,
    pub(super) foreground: Color,
    pub(super) muted: Color,
    pub(super) border: Color,
    pub(super) accent: Color,
    pub(super) selected_background: Color,
    pub(super) selected_foreground: Color,
    pub(super) critical: Color,
    pub(super) warning: Color,
    pub(super) context_low: Color,
    pub(super) context_medium: Color,
    pub(super) context_warning: Color,
    pub(super) context_critical: Color,
    pub(super) context_max: Color,
    pub(super) idle: Color,
    pub(super) busy: Color,
    pub(super) exited: Color,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum ThemeName {
    Catppuccin,
    Dracula,
    Gruvbox,
    Light,
    HighContrast,
    #[serde(rename = "sessionmesh", alias = "tokenplay", other)]
    #[default]
    SessionMesh,
}

impl ThemeName {
    const ALL: [Self; 6] = [
        Self::SessionMesh,
        Self::Catppuccin,
        Self::Dracula,
        Self::Gruvbox,
        Self::Light,
        Self::HighContrast,
    ];

    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::SessionMesh => "sessionmesh",
            Self::Catppuccin => "catppuccin",
            Self::Dracula => "dracula",
            Self::Gruvbox => "gruvbox",
            Self::Light => "light",
            Self::HighContrast => "high-contrast",
        }
    }

    pub(super) fn from_name(name: &str) -> Option<Self> {
        if name == "tokenplay" {
            return Some(Self::SessionMesh);
        }
        Self::ALL.into_iter().find(|theme| theme.as_str() == name)
    }

    pub(super) fn palette(self) -> Theme {
        match self {
            Self::SessionMesh => Theme {
                background: Color::Rgb(20, 22, 24),
                foreground: Color::Rgb(224, 220, 210),
                muted: Color::Rgb(132, 136, 141),
                border: Color::Rgb(92, 99, 106),
                accent: Color::Rgb(230, 180, 80),
                selected_background: Color::Rgb(68, 61, 43),
                selected_foreground: Color::Rgb(255, 245, 218),
                critical: Color::Rgb(245, 96, 96),
                warning: Color::Rgb(238, 190, 70),
                context_low: Color::Rgb(99, 179, 237),
                context_medium: Color::Rgb(120, 200, 154),
                context_warning: Color::Rgb(238, 190, 70),
                context_critical: Color::Rgb(255, 138, 91),
                context_max: Color::Rgb(255, 95, 109),
                idle: Color::Rgb(105, 184, 220),
                busy: Color::Rgb(120, 205, 120),
                exited: Color::Rgb(112, 116, 120),
            },
            Self::Catppuccin => Theme {
                background: Color::Rgb(30, 30, 46),
                foreground: Color::Rgb(205, 214, 244),
                muted: Color::Rgb(166, 173, 200),
                border: Color::Rgb(88, 91, 112),
                accent: Color::Rgb(203, 166, 247),
                selected_background: Color::Rgb(69, 71, 90),
                selected_foreground: Color::Rgb(245, 224, 220),
                critical: Color::Rgb(243, 139, 168),
                warning: Color::Rgb(249, 226, 175),
                context_low: Color::Rgb(116, 199, 236),
                context_medium: Color::Rgb(148, 226, 213),
                context_warning: Color::Rgb(249, 226, 175),
                context_critical: Color::Rgb(250, 179, 135),
                context_max: Color::Rgb(243, 139, 168),
                idle: Color::Rgb(137, 180, 250),
                busy: Color::Rgb(166, 227, 161),
                exited: Color::Rgb(108, 112, 134),
            },
            Self::Dracula => Theme {
                background: Color::Rgb(40, 42, 54),
                foreground: Color::Rgb(248, 248, 242),
                muted: Color::Rgb(166, 168, 184),
                border: Color::Rgb(98, 114, 164),
                accent: Color::Rgb(189, 147, 249),
                selected_background: Color::Rgb(68, 71, 90),
                selected_foreground: Color::Rgb(248, 248, 242),
                critical: Color::Rgb(255, 85, 85),
                warning: Color::Rgb(241, 250, 140),
                context_low: Color::Rgb(104, 214, 243),
                context_medium: Color::Rgb(125, 229, 138),
                context_warning: Color::Rgb(241, 250, 140),
                context_critical: Color::Rgb(255, 184, 108),
                context_max: Color::Rgb(255, 85, 85),
                idle: Color::Rgb(139, 233, 253),
                busy: Color::Rgb(80, 250, 123),
                exited: Color::Rgb(154, 156, 168),
            },
            Self::Gruvbox => Theme {
                background: Color::Rgb(40, 40, 40),
                foreground: Color::Rgb(235, 219, 178),
                muted: Color::Rgb(168, 153, 132),
                border: Color::Rgb(102, 92, 84),
                accent: Color::Rgb(250, 189, 47),
                selected_background: Color::Rgb(80, 73, 69),
                selected_foreground: Color::Rgb(251, 241, 199),
                critical: Color::Rgb(255, 105, 92),
                warning: Color::Rgb(250, 189, 47),
                context_low: Color::Rgb(142, 192, 124),
                context_medium: Color::Rgb(169, 198, 90),
                context_warning: Color::Rgb(250, 189, 47),
                context_critical: Color::Rgb(255, 155, 94),
                context_max: Color::Rgb(255, 105, 92),
                idle: Color::Rgb(131, 165, 152),
                busy: Color::Rgb(184, 187, 38),
                exited: Color::Rgb(146, 131, 116),
            },
            Self::Light => Theme {
                background: Color::Rgb(250, 249, 246),
                foreground: Color::Rgb(28, 31, 34),
                muted: Color::Rgb(96, 101, 106),
                border: Color::Rgb(126, 130, 133),
                accent: Color::Rgb(25, 91, 130),
                selected_background: Color::Rgb(20, 84, 116),
                selected_foreground: Color::Rgb(255, 255, 255),
                critical: Color::Rgb(176, 24, 32),
                warning: Color::Rgb(145, 92, 0),
                context_low: Color::Rgb(48, 95, 158),
                context_medium: Color::Rgb(20, 112, 92),
                context_warning: Color::Rgb(145, 92, 0),
                context_critical: Color::Rgb(179, 71, 24),
                context_max: Color::Rgb(176, 24, 32),
                idle: Color::Rgb(24, 91, 142),
                busy: Color::Rgb(28, 112, 55),
                exited: Color::Rgb(105, 105, 105),
            },
            Self::HighContrast => Theme {
                background: Color::Rgb(0, 0, 0),
                foreground: Color::Rgb(255, 255, 255),
                muted: Color::Rgb(190, 190, 190),
                border: Color::Rgb(255, 255, 255),
                accent: Color::Rgb(0, 255, 255),
                selected_background: Color::Rgb(255, 255, 255),
                selected_foreground: Color::Rgb(0, 0, 0),
                critical: Color::Rgb(255, 90, 90),
                warning: Color::Rgb(255, 255, 0),
                context_low: Color::Rgb(102, 191, 255),
                context_medium: Color::Rgb(181, 240, 111),
                context_warning: Color::Rgb(255, 255, 102),
                context_critical: Color::Rgb(255, 179, 71),
                context_max: Color::Rgb(255, 107, 107),
                idle: Color::Rgb(0, 255, 255),
                busy: Color::Rgb(80, 255, 80),
                exited: Color::Rgb(190, 190, 190),
            },
        }
    }
}

#[cfg(test)]
mod tests {
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
                ("context-low", theme.context_low, theme.background, 4.5),
                (
                    "context-medium",
                    theme.context_medium,
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
                ("context-max", theme.context_max, theme.background, 4.5),
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
                theme.context_low,
                theme.context_medium,
                theme.context_warning,
                theme.context_critical,
                theme.context_max,
            ];
            for pair in context_colors.windows(2) {
                assert_ne!(
                    pair[0],
                    pair[1],
                    "theme {} merges adjacent Context pressure bands",
                    theme_name.as_str(),
                );
            }
            for context_color in context_colors {
                assert_ne!(
                    context_color,
                    theme.idle,
                    "theme {} reuses Idle for Context pressure",
                    theme_name.as_str(),
                );
                assert_ne!(
                    context_color,
                    theme.busy,
                    "theme {} reuses Busy for Context pressure",
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
}
