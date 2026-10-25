use ratatui::{Frame, layout::Rect, style::Color};

const COLUMNS: usize = 80;
const ROWS: usize = 16;
const CELLS: usize = COLUMNS * ROWS;
const FPS: f64 = 30.0;
const FRAMES: usize = 91;
const FULL_MASS: f64 = 240.0;
const BACKGROUND: Color = Color::Rgb(18, 8, 28);
const WATER: Color = Color::Rgb(12, 110, 200);
const DOTS: [[u8; 2]; 4] = [[1, 8], [2, 16], [4, 32], [64, 128]];
const SAMPLES: &[u8; CELLS * (FRAMES + 1)] = include_bytes!("banner_fluid.bin");

pub(super) fn draw(frame: &mut Frame, area: Rect, elapsed: f64) {
    if area.width == 0 || area.height == 0 || !elapsed.is_finite() {
        return;
    }
    let height = area.height.min(4);
    let visible = Rect::new(area.x, area.y, area.width, height).intersection(frame.area());
    let mass = sample(elapsed);
    let buffer = frame.buffer_mut();
    for y in visible.y..visible.bottom() {
        for x in visible.x..visible.right() {
            let mut dots = 0_u8;
            let mut full = true;
            for (dy, bits) in DOTS.iter().enumerate() {
                let row = (usize::from(y - area.y) * 4 + dy) * ROWS / (usize::from(height) * 4);
                for (dx, bit) in bits.iter().enumerate() {
                    let column = (usize::from(x - area.x) * 2 + dx) * COLUMNS
                        / (usize::from(area.width) * 2);
                    let amount = mass(column, row);
                    if amount >= 32.0 {
                        dots |= *bit;
                    }
                    full &= amount >= FULL_MASS;
                }
            }
            let symbol = if full || dots == 0 {
                ' '
            } else {
                char::from_u32(0x2800 + u32::from(dots)).unwrap()
            };
            buffer[(x, y)]
                .set_char(symbol)
                .set_fg(WATER)
                .set_bg(if full { WATER } else { BACKGROUND });
        }
    }
}

pub(super) fn caption_ready(width: u16, text_width: usize, elapsed: f64) -> bool {
    let width = usize::from(width);
    if width == 0 || text_width == 0 || text_width > width || !elapsed.is_finite() {
        return false;
    }
    let left = (width - text_width) / 2;
    let first = left * COLUMNS / width;
    let end = ((left + text_width) * COLUMNS).div_ceil(width);
    let mass = sample(elapsed);
    (12..ROWS).all(|row| (first..end).all(|column| mass(column, row) >= FULL_MASS))
}

fn sample(elapsed: f64) -> impl Fn(usize, usize) -> f64 {
    let position = elapsed.clamp(0.0, (FRAMES - 1) as f64 / FPS) * FPS;
    let first = position as usize;
    let second = (first + 1).min(FRAMES - 1);
    let blend = position - first as f64;
    move |column, row| {
        let cell = row * COLUMNS + column;
        if SAMPLES[cell] != 0 {
            return 0.0;
        }
        let a = f64::from(SAMPLES[(first + 1) * CELLS + cell]);
        let b = f64::from(SAMPLES[(second + 1) * CELLS + cell]);
        a + (b - a) * blend
    }
}
