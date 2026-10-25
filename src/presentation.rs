use ratatui::{
    Frame,
    layout::{Direction, Rect},
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

pub(crate) fn grass_stripes(
    frame: &mut Frame,
    plot: Rect,
    pitch: [f64; 2],
    world: [f64; 2],
    direction: Direction,
) {
    let axis = if direction == Direction::Horizontal {
        1
    } else {
        0
    };
    for row in 0..plot.height {
        for column in 0..plot.width {
            let position = [
                (f64::from(column) + 0.5) / f64::from(plot.width) - 0.5,
                0.5 - (f64::from(row) + 0.5) / f64::from(plot.height),
            ];
            let point = [position[0] * world[0], position[1] * world[1]];
            if point[0].abs() > pitch[0] / 2.0 || point[1].abs() > pitch[1] / 2.0 {
                continue;
            }
            let band = (((point[axis] / pitch[axis] + 0.5) * 12.0).floor() as usize).min(11);
            let color = if band % 2 == 0 {
                Color::Rgb(18, 52, 33)
            } else {
                Color::Rgb(25, 70, 43)
            };
            frame.buffer_mut()[(plot.x + column, plot.y + row)].set_bg(color);
        }
    }
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
