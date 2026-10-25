use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
};

const WIDTH: u8 = 5;
const HEIGHT: u8 = 7;
const FONT: &[(u8, &[[u8; 4]])] = &[
    (b'T', &[[27, 31, 32, 33], [7, 12, 17, 22], [0, 1, 2, 3]]),
    (
        b'E',
        &[
            [31, 32, 33, 34],
            [20, 21, 25, 30],
            [15, 16, 17, 18],
            [0, 5, 6, 10],
            [1, 2, 3, 4],
        ],
    ),
    (
        b'S',
        &[
            [30, 31, 32, 33],
            [23, 24, 28, 29],
            [11, 16, 17, 18],
            [0, 5, 6, 10],
            [1, 2, 3, 4],
        ],
    ),
    (
        b'C',
        &[
            [20, 25, 26, 31],
            [5, 6, 10, 15],
            [28, 29, 32, 33],
            [0, 1, 2, 3],
        ],
    ),
    (
        b'O',
        &[
            [25, 26, 31, 32],
            [5, 10, 15, 20],
            [24, 28, 29, 33],
            [0, 1, 2, 3],
            [4, 9, 14, 19],
        ],
    ),
    (
        b'M',
        &[
            [19, 24, 29, 34],
            [4, 8, 9, 14],
            [15, 20, 25, 30],
            [10, 11, 12, 17],
            [0, 1, 5, 6],
        ],
    ),
    (
        b'P',
        &[
            [15, 20, 25, 30],
            [10, 11, 16, 17],
            [0, 1, 5, 6],
            [12, 13, 14, 18],
            [2, 3, 4, 9],
        ],
    ),
    (b'L', &[[15, 20, 25, 30], [31, 32, 33, 34], [0, 1, 5, 10]]),
];

const _: () = {
    let mut letter = 0;
    while letter < FONT.len() {
        let pieces = FONT[letter].1;
        let mut board = 0_u64;
        let mut index = 0;
        while index < pieces.len() {
            let piece = pieces[index];
            let mut mask = 0_u64;
            let mut edges = 0;
            let mut i = 0;
            while i < 4 {
                assert!(piece[i] < WIDTH * HEIGHT);
                let bit = 1_u64 << piece[i];
                assert!(mask & bit == 0);
                mask |= bit;
                let mut j = i + 1;
                while j < 4 {
                    let dx = (piece[i] % WIDTH).abs_diff(piece[j] % WIDTH);
                    let dy = (piece[i] / WIDTH).abs_diff(piece[j] / WIDTH);
                    if dx + dy == 1 {
                        edges += 1;
                    }
                    j += 1;
                }
                i += 1;
            }
            assert!(edges >= 3);
            let mut offset = -(HEIGHT as i16);
            while offset <= 0 {
                i = 0;
                while i < 4 {
                    let cell = piece[i] as i16 + offset * WIDTH as i16;
                    if cell >= 0 {
                        assert!(board & (1_u64 << cell) == 0);
                    }
                    i += 1;
                }
                offset += 1;
            }
            let mut supported = false;
            i = 0;
            while i < 4 {
                let below = piece[i] + WIDTH;
                if below >= WIDTH * HEIGHT || board & (1_u64 << below) != 0 {
                    supported = true;
                }
                i += 1;
            }
            assert!(supported);
            board |= mask;
            index += 1;
        }
        letter += 1;
    }
};

fn glyph(letter: u8) -> &'static [[u8; 4]] {
    FONT.iter()
        .find(|(key, _)| *key == letter)
        .map_or(&[], |(_, pieces)| *pieces)
}

fn advance(letter: u8) -> usize {
    glyph(letter)
        .iter()
        .flatten()
        .map(|cell| usize::from(cell % WIDTH) + 2)
        .max()
        .unwrap_or(3)
}

pub(super) fn draw(frame: &mut Frame, area: Rect, title: &str, elapsed: f64) {
    if area.width == 0 || area.height < 4 {
        return;
    }
    let width = usize::from(area.width);
    let total = title.bytes().map(advance).sum::<usize>().saturating_sub(1);
    let travel = total.saturating_sub(width);
    let camera = (((elapsed - 3.7) / 4.3).clamp(0.0, 1.0) * travel as f64) as i32;
    let mut origin = width.saturating_sub(total) as i32 / 2 - camera;
    let background = Color::Rgb(18, 8, 28);
    let palette = [
        Color::LightCyan,
        Color::LightYellow,
        Color::LightMagenta,
        Color::LightGreen,
        Color::Rgb(255, 150, 45),
        Color::LightBlue,
        Color::LightRed,
    ];
    let mut cells = vec![background; width * 8];
    for (letter_index, letter) in title.bytes().enumerate() {
        for (index, piece) in glyph(letter).iter().enumerate() {
            let age = elapsed - letter_index as f64 * 0.15 - index as f64 * 0.7;
            if age < 0.0 {
                continue;
            }
            let progress = (age / 0.6).min(1.0);
            let top = -1 - i32::from(piece.iter().max().unwrap() / WIDTH);
            let offset = (f64::from(top) * (1.0 - progress * progress)).floor() as i32;
            let color = palette[(letter_index * 3 + index) % palette.len()];
            for &cell in piece {
                let x = origin + i32::from(cell % WIDTH);
                let y = i32::from(cell / WIDTH) + offset;
                if (0..width as i32).contains(&x) && (0..i32::from(HEIGHT)).contains(&y) {
                    cells[y as usize * width + x as usize] = color;
                }
            }
        }
        origin += advance(letter) as i32;
    }
    for x in 0..width {
        cells[7 * width + x] = Color::DarkGray;
    }
    for row in 0..4 {
        for x in 0..width {
            frame.buffer_mut()[(area.x + x as u16, area.y + row as u16)]
                .set_symbol("▀")
                .set_style(
                    Style::default()
                        .fg(cells[row * 2 * width + x])
                        .bg(cells[(row * 2 + 1) * width + x]),
                );
        }
    }
}
