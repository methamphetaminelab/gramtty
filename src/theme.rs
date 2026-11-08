use ratatui::style::{Color, Modifier, Style};

pub const SELECTION_MARKER: &str = "❯ ";
pub const ORANGE: Color = Color::Rgb(255, 165, 0);

pub fn normal() -> Style {
    Style::default()
}

pub fn strong() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}

pub fn muted() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}

pub fn border() -> Style {
    Style::default().fg(Color::DarkGray)
}

pub fn focus_border() -> Style {
    Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD)
}

pub fn selection() -> Style {
    Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD)
        .remove_modifier(Modifier::DIM)
}

pub fn selection_border() -> Style {
    Style::default().fg(Color::Green)
}

pub fn header_title() -> Style {
    Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD)
}

pub fn active_field() -> Style {
    Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD)
}

pub fn error() -> Style {
    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
}

pub fn warning() -> Style {
    Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD)
}

pub fn editing() -> Style {
    Style::default().fg(Color::Yellow)
}

pub fn unread() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}

pub fn mention_badge() -> Style {
    Style::default().fg(ORANGE)
}

pub fn link() -> Style {
    Style::default()
        .fg(Color::Blue)
        .add_modifier(Modifier::UNDERLINED)
}

pub fn mention() -> Style {
    Style::default().fg(Color::Cyan)
}

pub fn code() -> Style {
    Style::default().fg(Color::Rgb(230, 170, 90))
}

pub fn spoiler() -> Style {
    Style::default().fg(Color::DarkGray).bg(Color::DarkGray)
}

pub fn media() -> Style {
    Style::default()
        .fg(Color::Magenta)
        .add_modifier(Modifier::ITALIC)
}
