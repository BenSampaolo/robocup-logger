use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Clear, Padding},
};

pub(crate) const FOCUS: Color = Color::LightYellow;
pub(crate) const ACCENT: Color = Color::Cyan;
pub(crate) const MUTED: Color = Color::DarkGray;
pub(crate) const SUCCESS: Color = Color::Green;
pub(crate) const ERROR: Color = Color::Red;

pub(crate) fn solid_panel(frame: &mut Frame, area: Rect, block: Block<'_>, accent: Color) -> Rect {
    let area = area.intersection(frame.area());
    let border_area = Rect::new(
        area.x + area.width.min(1),
        area.y,
        area.width.saturating_sub(2),
        area.height,
    );
    // Equal foreground/background fills the whole cell, independently of the line glyph/font.
    let band = Style::default().fg(accent).bg(accent);
    let block = block
        .borders(Borders::ALL)
        .style(Style::default().fg(Color::White).bg(Color::Black))
        .border_style(band)
        .title_style(
            Style::default()
                .fg(Color::Black)
                .bg(accent)
                .add_modifier(Modifier::BOLD),
        );
    let inner = block.inner(border_area);
    frame.render_widget(Clear, area);
    frame.render_widget(Block::default().style(band), area);
    frame.render_widget(block, border_area);
    inner
}

pub(crate) fn popup(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    accent: Color,
    width: u16,
    height: u16,
) -> Rect {
    let area = area.intersection(frame.area());
    frame.buffer_mut().set_style(
        area,
        Style::default()
            .fg(MUTED)
            .bg(Color::Black)
            .remove_modifier(Modifier::BOLD | Modifier::REVERSED),
    );
    // Grow outward for the wider sides and padding; keep the original content width when possible.
    let width = width.saturating_add(2).min(area.width);
    let horizontal = (area.width.saturating_sub(width) / 2).min(2);
    let vertical = (area.height.saturating_sub(height) / 2).min(1);
    let width = width.saturating_add(horizontal * 2).min(area.width);
    let height = height.saturating_add(vertical * 2).min(area.height);
    let rect = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    let block = Block::bordered()
        .padding(Padding::new(horizontal, horizontal, vertical, vertical))
        .title(title);
    solid_panel(frame, rect, block, accent)
}
