use ratatui::style::Color;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy)]
pub(super) struct Theme {
    pub(super) background: Color,
    pub(super) foreground: Color,
    pub(super) muted: Color,
    pub(super) border: Color,
    pub(super) accent: Color,
    pub(super) provider_claude: Color,
    pub(super) provider_codex: Color,
    pub(super) selected_background: Color,
    pub(super) selected_foreground: Color,
    pub(super) critical: Color,
    pub(super) warning: Color,
    pub(super) context_normal: Color,
    pub(super) context_warning: Color,
    pub(super) context_critical: Color,
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
    #[serde(rename = "mudi", other)]
    #[default]
    Mudi,
}

impl ThemeName {
    #[cfg(test)]
    const ALL: [Self; 6] = [
        Self::Mudi,
        Self::Catppuccin,
        Self::Dracula,
        Self::Gruvbox,
        Self::Light,
        Self::HighContrast,
    ];

    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Mudi => "mudi",
            Self::Catppuccin => "catppuccin",
            Self::Dracula => "dracula",
            Self::Gruvbox => "gruvbox",
            Self::Light => "light",
            Self::HighContrast => "high-contrast",
        }
    }

    pub(super) fn palette(self) -> Theme {
        match self {
            Self::Mudi => Theme {
                background: Color::Rgb(20, 22, 24),
                foreground: Color::Rgb(224, 220, 210),
                muted: Color::Rgb(132, 136, 141),
                border: Color::Rgb(92, 99, 106),
                accent: Color::Rgb(230, 180, 80),
                provider_claude: Color::Rgb(217, 119, 87),
                provider_codex: Color::Rgb(122, 157, 255),
                selected_background: Color::Rgb(32, 35, 38),
                selected_foreground: Color::Rgb(255, 245, 218),
                critical: Color::Rgb(245, 96, 96),
                warning: Color::Rgb(238, 190, 70),
                context_normal: Color::Rgb(120, 200, 154),
                context_warning: Color::Rgb(238, 190, 70),
                context_critical: Color::Rgb(255, 95, 109),
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
                provider_claude: Color::Rgb(217, 119, 87),
                provider_codex: Color::Rgb(122, 157, 255),
                selected_background: Color::Rgb(35, 36, 52),
                selected_foreground: Color::Rgb(245, 224, 220),
                critical: Color::Rgb(243, 139, 168),
                warning: Color::Rgb(249, 226, 175),
                context_normal: Color::Rgb(148, 226, 213),
                context_warning: Color::Rgb(249, 226, 175),
                context_critical: Color::Rgb(243, 139, 168),
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
                provider_claude: Color::Rgb(217, 119, 87),
                provider_codex: Color::Rgb(122, 157, 255),
                selected_background: Color::Rgb(34, 38, 62),
                selected_foreground: Color::Rgb(248, 248, 242),
                critical: Color::Rgb(255, 85, 85),
                warning: Color::Rgb(241, 250, 140),
                context_normal: Color::Rgb(125, 229, 138),
                context_warning: Color::Rgb(241, 250, 140),
                context_critical: Color::Rgb(255, 85, 85),
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
                provider_claude: Color::Rgb(217, 119, 87),
                provider_codex: Color::Rgb(122, 157, 255),
                selected_background: Color::Rgb(47, 36, 29),
                selected_foreground: Color::Rgb(251, 241, 199),
                critical: Color::Rgb(255, 105, 92),
                warning: Color::Rgb(250, 189, 47),
                context_normal: Color::Rgb(169, 198, 90),
                context_warning: Color::Rgb(250, 189, 47),
                context_critical: Color::Rgb(255, 105, 92),
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
                provider_claude: Color::Rgb(161, 75, 52),
                provider_codex: Color::Rgb(57, 65, 255),
                selected_background: Color::Rgb(230, 233, 235),
                selected_foreground: Color::Rgb(28, 31, 34),
                critical: Color::Rgb(176, 24, 32),
                warning: Color::Rgb(145, 92, 0),
                context_normal: Color::Rgb(20, 112, 92),
                context_warning: Color::Rgb(145, 92, 0),
                context_critical: Color::Rgb(176, 24, 32),
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
                provider_claude: Color::Rgb(217, 119, 87),
                provider_codex: Color::Rgb(122, 157, 255),
                selected_background: Color::Rgb(20, 20, 20),
                selected_foreground: Color::Rgb(255, 255, 255),
                critical: Color::Rgb(255, 90, 90),
                warning: Color::Rgb(255, 255, 0),
                context_normal: Color::Rgb(181, 240, 111),
                context_warning: Color::Rgb(255, 255, 102),
                context_critical: Color::Rgb(255, 107, 107),
                idle: Color::Rgb(0, 255, 255),
                busy: Color::Rgb(80, 255, 80),
                exited: Color::Rgb(190, 190, 190),
            },
        }
    }
}

#[cfg(test)]
#[path = "theme_test.rs"]
mod tests;
