use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders};

use crate::syntax::HighlightKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Theme {
    pub name: &'static str,
    pub bg: Color,
    pub bg_current: Color,
    pub bg_selected: Color,
    pub bg_status: Color,
    pub fg: Color,
    pub fg_dim: Color,
    pub fg_muted: Color,
    pub comment: Color,
    pub keyword: Color,
    pub string: Color,
    pub number: Color,
    pub type_name: Color,
    pub function: Color,
    pub property: Color,
    pub constant: Color,
    pub operator: Color,
    pub border: Color,
    pub accent: Color,
    pub folder: Color,
    pub added: Color,
    pub modified: Color,
    pub deleted: Color,
    pub warning: Color,
    pub search: Color,
    pub search_current: Color,
    pub on_accent: Color,
}

impl Theme {
    pub fn text(self) -> Style {
        Style::default().bg(self.bg).fg(self.fg)
    }

    pub fn panel<'a>(self, title: &'a str, focused: bool) -> Block<'a> {
        let color = if focused { self.accent } else { self.border };
        Block::default()
            .title(title)
            .title_style(Style::default().fg(if focused { self.accent } else { self.fg_muted }))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(color))
            .style(self.text())
    }
}

pub fn theme_names() -> [&'static str; 5] {
    ["p0", "light", "contrast", "ocean", "ember"]
}

pub fn by_name(name: &str) -> Theme {
    match name {
        "light" => LIGHT,
        "contrast" => CONTRAST,
        "ocean" => OCEAN,
        "ember" => EMBER,
        _ => P0,
    }
}

const P0: Theme = Theme {
    name: "p0",
    bg: Color::Rgb(30, 30, 30),
    bg_current: Color::Rgb(40, 40, 40),
    bg_selected: Color::Rgb(38, 79, 120),
    bg_status: Color::Rgb(48, 48, 48),
    fg: Color::Rgb(220, 220, 220),
    fg_dim: Color::Rgb(128, 128, 128),
    fg_muted: Color::Rgb(160, 160, 160),
    comment: Color::Rgb(164, 170, 164),
    keyword: Color::Rgb(197, 134, 192),
    string: Color::Rgb(122, 176, 122),
    number: Color::Rgb(110, 186, 196),
    type_name: Color::Rgb(220, 186, 104),
    function: Color::Rgb(106, 156, 214),
    property: Color::Rgb(140, 186, 220),
    constant: Color::Rgb(128, 210, 214),
    operator: Color::Rgb(168, 168, 168),
    border: Color::Rgb(90, 90, 90),
    accent: Color::Rgb(110, 186, 196),
    folder: Color::Rgb(196, 208, 214),
    added: Color::Rgb(46, 160, 67),
    modified: Color::Rgb(56, 132, 214),
    deleted: Color::Rgb(214, 72, 72),
    warning: Color::Rgb(220, 186, 80),
    search: Color::Rgb(92, 78, 28),
    search_current: Color::Rgb(220, 186, 80),
    on_accent: Color::Rgb(20, 20, 20),
};

const LIGHT: Theme = Theme {
    name: "light",
    bg: Color::Rgb(246, 246, 246),
    bg_current: Color::Rgb(232, 236, 240),
    bg_selected: Color::Rgb(186, 214, 236),
    bg_status: Color::Rgb(228, 228, 228),
    fg: Color::Rgb(31, 35, 40),
    fg_dim: Color::Rgb(110, 118, 129),
    fg_muted: Color::Rgb(90, 98, 108),
    comment: Color::Rgb(106, 115, 125),
    keyword: Color::Rgb(130, 80, 223),
    string: Color::Rgb(10, 48, 105),
    number: Color::Rgb(5, 80, 100),
    type_name: Color::Rgb(149, 56, 0),
    function: Color::Rgb(102, 57, 186),
    property: Color::Rgb(5, 80, 174),
    constant: Color::Rgb(0, 92, 110),
    operator: Color::Rgb(80, 80, 80),
    border: Color::Rgb(180, 180, 180),
    accent: Color::Rgb(0, 92, 150),
    folder: Color::Rgb(36, 72, 104),
    added: Color::Rgb(20, 120, 50),
    modified: Color::Rgb(20, 80, 170),
    deleted: Color::Rgb(180, 30, 40),
    warning: Color::Rgb(140, 90, 0),
    search: Color::Rgb(255, 230, 160),
    search_current: Color::Rgb(240, 190, 40),
    on_accent: Color::Rgb(20, 20, 20),
};

const CONTRAST: Theme = Theme {
    name: "contrast",
    bg: Color::Rgb(0, 0, 0),
    bg_current: Color::Rgb(24, 24, 24),
    bg_selected: Color::Rgb(0, 80, 160),
    bg_status: Color::Rgb(16, 16, 16),
    fg: Color::Rgb(255, 255, 255),
    fg_dim: Color::Rgb(190, 190, 190),
    fg_muted: Color::Rgb(210, 210, 210),
    comment: Color::Rgb(176, 176, 176),
    keyword: Color::Rgb(255, 120, 255),
    string: Color::Rgb(255, 200, 80),
    number: Color::Rgb(120, 255, 255),
    type_name: Color::Rgb(255, 230, 80),
    function: Color::Rgb(120, 180, 255),
    property: Color::Rgb(160, 210, 255),
    constant: Color::Rgb(140, 255, 220),
    operator: Color::Rgb(220, 220, 220),
    border: Color::Rgb(200, 200, 200),
    accent: Color::Rgb(80, 200, 255),
    folder: Color::Rgb(200, 230, 255),
    added: Color::Rgb(0, 220, 80),
    modified: Color::Rgb(80, 160, 255),
    deleted: Color::Rgb(255, 70, 70),
    warning: Color::Rgb(255, 220, 0),
    search: Color::Rgb(80, 70, 0),
    search_current: Color::Rgb(255, 220, 0),
    on_accent: Color::Rgb(0, 0, 0),
};

const OCEAN: Theme = Theme {
    name: "ocean",
    bg: Color::Rgb(11, 30, 45),
    bg_current: Color::Rgb(16, 42, 60),
    bg_selected: Color::Rgb(28, 72, 110),
    bg_status: Color::Rgb(8, 24, 36),
    fg: Color::Rgb(214, 230, 242),
    fg_dim: Color::Rgb(110, 136, 156),
    fg_muted: Color::Rgb(150, 172, 188),
    comment: Color::Rgb(127, 152, 171),
    keyword: Color::Rgb(127, 209, 200),
    string: Color::Rgb(230, 192, 123),
    number: Color::Rgb(130, 200, 220),
    type_name: Color::Rgb(130, 170, 255),
    function: Color::Rgb(195, 232, 141),
    property: Color::Rgb(150, 200, 230),
    constant: Color::Rgb(140, 220, 210),
    operator: Color::Rgb(160, 180, 196),
    border: Color::Rgb(60, 90, 110),
    accent: Color::Rgb(127, 209, 200),
    folder: Color::Rgb(190, 214, 226),
    added: Color::Rgb(80, 190, 120),
    modified: Color::Rgb(90, 160, 230),
    deleted: Color::Rgb(230, 100, 100),
    warning: Color::Rgb(230, 192, 100),
    search: Color::Rgb(70, 80, 30),
    search_current: Color::Rgb(230, 192, 100),
    on_accent: Color::Rgb(11, 30, 45),
};

const EMBER: Theme = Theme {
    name: "ember",
    bg: Color::Rgb(34, 28, 24),
    bg_current: Color::Rgb(48, 40, 34),
    bg_selected: Color::Rgb(92, 62, 40),
    bg_status: Color::Rgb(28, 22, 18),
    fg: Color::Rgb(241, 230, 214),
    fg_dim: Color::Rgb(150, 134, 118),
    fg_muted: Color::Rgb(186, 170, 154),
    comment: Color::Rgb(168, 152, 136),
    keyword: Color::Rgb(224, 159, 109),
    string: Color::Rgb(214, 196, 138),
    number: Color::Rgb(220, 170, 120),
    type_name: Color::Rgb(232, 180, 160),
    function: Color::Rgb(240, 210, 168),
    property: Color::Rgb(220, 190, 160),
    constant: Color::Rgb(230, 200, 140),
    operator: Color::Rgb(180, 164, 148),
    border: Color::Rgb(90, 74, 62),
    accent: Color::Rgb(224, 159, 109),
    folder: Color::Rgb(230, 210, 190),
    added: Color::Rgb(120, 180, 90),
    modified: Color::Rgb(120, 150, 210),
    deleted: Color::Rgb(210, 90, 70),
    warning: Color::Rgb(224, 170, 80),
    search: Color::Rgb(90, 64, 28),
    search_current: Color::Rgb(224, 170, 80),
    on_accent: Color::Rgb(34, 28, 24),
};

pub(crate) fn highlight_color(kind: HighlightKind, theme: Theme) -> Color {
    match kind {
        HighlightKind::Comment => theme.comment,
        HighlightKind::String => theme.string,
        HighlightKind::Number => theme.number,
        HighlightKind::Keyword => theme.keyword,
        HighlightKind::Type => theme.type_name,
        HighlightKind::Function => theme.function,
        HighlightKind::Property => theme.property,
        HighlightKind::Constant => theme.constant,
        HighlightKind::Operator => theme.operator,
        HighlightKind::Variable => theme.fg,
        HighlightKind::Heading => theme.type_name,
        HighlightKind::Emphasis | HighlightKind::Strong => theme.fg,
        HighlightKind::Strike => theme.fg_dim,
        HighlightKind::Link => theme.accent,
    }
}

pub(crate) fn highlight_modifier(kind: HighlightKind) -> Modifier {
    match kind {
        HighlightKind::Heading | HighlightKind::Strong => Modifier::BOLD,
        HighlightKind::Emphasis => Modifier::ITALIC,
        HighlightKind::Strike => Modifier::CROSSED_OUT,
        HighlightKind::Link => Modifier::UNDERLINED,
        _ => Modifier::empty(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_names_resolve_and_unknown_names_fall_back() {
        assert_eq!(by_name("ocean").name, "ocean");
        assert_eq!(by_name("light").name, "light");
        assert_eq!(by_name("contrast").name, "contrast");
        assert_eq!(by_name("ember").name, "ember");
        assert_eq!(by_name("p0").name, "p0");
        assert_eq!(by_name("nope").name, "p0");
    }

    #[test]
    fn comments_differ_from_each_background() {
        for name in theme_names() {
            let theme = by_name(name);
            assert_ne!(theme.comment, theme.bg, "{name}");
            assert_ne!(theme.folder, theme.fg_muted, "{name}");
        }
    }
}
