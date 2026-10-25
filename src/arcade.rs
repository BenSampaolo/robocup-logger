use std::time::{Duration, Instant};

use ratatui::{
    Frame,
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Paragraph},
};

#[path = "fluid.rs"]
mod fluid;
#[path = "tetris.rs"]
mod tetris;

const LIFETIME: Duration = Duration::from_secs(3);

#[derive(Clone, Copy)]
pub(crate) enum Effect {
    Ticker,
    Speed,
    Whistle,
    Mirror,
    Fireworks,
    Ripple,
    Aurora,
    Spotlight,
    Sparkle,
    Scan,
    Orbit,
    Concede,
    Flame,
    Split,
    Tetris,
    Laser,
    Scramble,
    Target,
    Launch,
    Ricochet,
    Gate,
    Freeze,
    Noise,
    Fluid,
    Clock,
}

pub(crate) struct Cue {
    pub effect: Effect,
    pub title: String,
    pub detail: String,
    pub color: Color,
    pub started: Instant,
}

pub(crate) fn draw(
    frame: &mut Frame,
    area: Rect,
    cue: Option<&Cue>,
    now: Instant,
    gc_clock: Option<String>,
) {
    if area.width == 0 || area.height < 4 {
        return;
    }
    let Some(cue) = cue.filter(|cue| now.duration_since(cue.started) < cue.duration()) else {
        let (title, label) = match gc_clock {
            Some(clock) => (clock, "GC CLOCK"),
            None => (
                chrono::Local::now()
                    .format(if area.width >= 32 {
                        "%H:%M:%S"
                    } else {
                        "%H:%M"
                    })
                    .to_string(),
                "LOCAL TIME",
            ),
        };
        let clock = Cue {
            effect: Effect::Clock,
            title,
            detail: String::new(),
            color: Color::Cyan,
            started: now,
        };
        draw_letters(
            frame,
            Rect::new(area.x, area.y, area.width, 3),
            &clock,
            0.0,
            false,
        );
        frame.render_widget(
            Paragraph::new(label)
                .alignment(Alignment::Center)
                .style(Style::default().fg(Color::DarkGray)),
            Rect::new(area.x, area.y + 3, area.width, 1),
        );
        return;
    };
    let elapsed = now.duration_since(cue.started).as_secs_f64();
    let fade = cue.fading(elapsed);
    frame.render_widget(
        Block::default().style(Style::default().bg(Color::Rgb(18, 8, 28))),
        area,
    );
    if matches!(cue.effect, Effect::Tetris) {
        tetris::draw(frame, area, &cue.title, elapsed);
        return;
    }
    if matches!(cue.effect, Effect::Fluid) {
        fluid::draw(frame, area, elapsed);
        let caption = Line::raw(cue.detail.as_str());
        let text_width = caption.width().min(usize::from(area.width));
        if fluid::caption_ready(area.width, text_width, elapsed) {
            frame.render_widget(
                Paragraph::new(caption)
                    .alignment(Alignment::Center)
                    .style(Style::default().fg(Color::White)),
                Rect::new(
                    area.x + (area.width - text_width as u16) / 2,
                    area.y + 3,
                    text_width as u16,
                    1,
                ),
            );
        }
        return;
    }
    draw_effect(frame, area, cue, elapsed);
    let whistle_intro = matches!(cue.effect, Effect::Whistle) && elapsed < 0.55;
    draw_letters(
        frame,
        Rect::new(
            area.x,
            area.y,
            area.width,
            if whistle_intro { 4 } else { 3 },
        ),
        cue,
        elapsed,
        false,
    );
    if matches!(cue.effect, Effect::Mirror) && elapsed < 1.5 {
        draw_letters(
            frame,
            Rect::new(area.x, area.y + 3, area.width, 1),
            cue,
            elapsed,
            true,
        );
    } else if !whistle_intro {
        frame.render_widget(
            Paragraph::new(cue.detail.as_str())
                .alignment(Alignment::Center)
                .style(Style::default().fg(if fade { Color::DarkGray } else { Color::White })),
            Rect::new(area.x, area.y + 3, area.width, 1),
        );
    }
}

fn draw_effect(frame: &mut Frame, area: Rect, cue: &Cue, elapsed: f64) {
    let width = i32::from(area.width);
    let tick = (elapsed * 10.0) as i32;
    let colors = [
        cue.color,
        Color::LightCyan,
        Color::LightMagenta,
        Color::LightYellow,
    ];
    let mut paint = |x: i32, y: i32, symbol: &str, color: Color| {
        if (0..width).contains(&x) && (0..4).contains(&y) {
            frame.buffer_mut()[(area.x + x as u16, area.y + y as u16)]
                .set_symbol(symbol)
                .set_fg(if cue.fading(elapsed) {
                    Color::DarkGray
                } else {
                    color
                });
        }
    };
    match cue.effect {
        Effect::Clock => {}
        Effect::Ripple => {
            for ring in 0..3 {
                let radius = (elapsed * 0.65 + f64::from(ring) * 0.45) % 1.4;
                for ray in 0..40 {
                    let (sin, cos) = (f64::from(ray) * std::f64::consts::TAU / 40.0).sin_cos();
                    paint(
                        (f64::from(width) * (0.5 + radius * cos / 2.0)).round() as i32,
                        (1.5 + radius * 2.5 * sin).round() as i32,
                        "⠶",
                        spectrum(radius * 4.0),
                    );
                }
            }
        }
        Effect::Aurora => {
            for x in 0..width {
                for y in 0..4 {
                    let phase = elapsed * 2.0 + f64::from(x) * 0.18 + f64::from(y);
                    let symbols = ["⣀", "⣤", "⣶", "⣿"];
                    paint(
                        x,
                        y,
                        symbols[((phase.sin() + 1.0) * 1.5) as usize],
                        spectrum(phase),
                    );
                }
            }
        }
        Effect::Spotlight => {
            let center = f64::from(width) * (0.5 + 0.4 * (elapsed * 1.8).sin());
            for y in 0..4 {
                for offset in -y..=y {
                    paint(
                        center as i32 + offset,
                        y,
                        "⣿",
                        spectrum(elapsed + f64::from(offset) * 0.3),
                    );
                    paint(width - center as i32 + offset, y, "⡇", Color::LightMagenta);
                }
            }
        }
        Effect::Sparkle => {
            for star in 0..width.min(36) {
                let phase = elapsed * 2.5 + f64::from(star);
                let symbols = ["⠂", "⠐", "⠶", "⠿", "⠶", "⠐"];
                paint(
                    (star * 13) % width,
                    (star * 7) % 4,
                    symbols[(phase as usize) % symbols.len()],
                    spectrum(phase * 0.4),
                );
            }
        }
        Effect::Scan => {
            let head = (elapsed * 18.0) as i32 % (width + 6);
            for row in 0..4 {
                for tail in 0..6 {
                    paint(
                        head - tail,
                        row,
                        if tail == 0 { "⣿" } else { "⡇" },
                        if tail == 0 {
                            Color::White
                        } else {
                            Color::Rgb(30, 210 - tail as u8 * 20, 230)
                        },
                    );
                }
            }
        }
        Effect::Orbit => {
            for particle in 0..18 {
                let angle = elapsed * 1.6 + f64::from(particle) * std::f64::consts::TAU / 18.0;
                paint(
                    (f64::from(width) * (0.5 + angle.cos() * 0.48)).round() as i32,
                    (1.5 + angle.sin() * 1.5).round() as i32,
                    "⠿",
                    spectrum(angle),
                );
            }
        }
        Effect::Concede => {
            for drop in 0..width.min(24) {
                paint(
                    (drop * 11) % width,
                    (drop + tick) % 4,
                    "⡇",
                    Color::Rgb(220, 55, 90),
                );
            }
        }
        Effect::Flame => {
            for x in 0..width {
                let heat = (elapsed * 5.0 + f64::from(x) * 1.7).sin();
                let height = (heat + 1.0) as i32 + 1;
                for y in (4 - height)..4 {
                    paint(
                        x,
                        y,
                        if y == 4 - height { "⣄" } else { "⣿" },
                        Color::Rgb(255, (80.0 + 80.0 * (heat + 1.0)) as u8, 15),
                    );
                }
            }
        }
        Effect::Split => {
            for row in 0..4 {
                let x = (tick * 3 + row * 5) % (width / 2 + 1);
                paint(x, row, "⢼", Color::LightCyan);
                paint(width - 1 - x, row, "⡧", Color::LightBlue);
            }
        }
        Effect::Tetris | Effect::Fluid => {}
        Effect::Laser => {
            let radius = (elapsed * 24.0) as i32;
            for x in (width / 2 - radius)..=(width / 2 + radius) {
                paint(
                    x,
                    3,
                    if (x + tick) % 4 == 0 { "⣿" } else { "⠒" },
                    Color::LightYellow,
                );
            }
        }
        Effect::Scramble => {
            for row in 0..4 {
                for block in 0..width / 4 + 1 {
                    let x = block * 4 + (tick + row).rem_euclid(4);
                    paint(
                        x,
                        row,
                        "⠿",
                        if (block + tick) % 2 == 0 {
                            Color::LightMagenta
                        } else {
                            Color::LightCyan
                        },
                    );
                }
            }
        }
        Effect::Target => {
            let inset = (elapsed.min(0.7) / 0.7 * f64::from(width / 4)) as i32;
            for row in 0..4 {
                for x in [inset, width - 1 - inset] {
                    paint(
                        x,
                        row,
                        if row == 0 || row == 3 { "⣇" } else { "⡇" },
                        Color::LightRed,
                    );
                }
            }
        }
        Effect::Launch => {
            for x in 0..width {
                if x % 5 == 0 {
                    paint(x, 3 - (x + tick * 2).rem_euclid(4), "⣀", Color::LightGreen);
                }
            }
        }
        Effect::Ricochet => {
            let travel = (elapsed * 20.0) as i32 % (width + 6);
            let (x, y) = if travel < 4 {
                (0, 3 - travel)
            } else {
                (travel - 4, 0)
            };
            for tail in 0..5 {
                paint(x - tail, y, "⣿", Color::Rgb(255, 165 - tail as u8 * 20, 35));
            }
        }
        Effect::Gate => {
            let gap = (elapsed.min(0.7) / 0.7 * f64::from(width / 2)) as i32;
            for x in 0..width {
                if (x - width / 2).abs() >= gap {
                    paint(x, 0, "⣤", Color::LightYellow);
                    paint(x, 3, "⠛", Color::LightYellow);
                }
            }
        }
        Effect::Freeze => {
            let offset = ((1.0 - elapsed / 0.4).max(0.0) * f64::from(width / 2)) as i32;
            for row in 0..4 {
                paint(offset, row, "⣿", Color::LightRed);
                paint(width - 1 - offset, row, "⣿", Color::LightRed);
            }
        }
        Effect::Noise => {}
        Effect::Ticker => {
            let flakes = ["⠁", "⠄", "⡀", "⠂", "⠐", "⠈", "⠳"];
            for particle in 0..width.min(48) {
                let x = (particle * 17 + tick * (particle % 3 + 1)) % width;
                let y = (particle * 7 + tick / 2) % 4;
                paint(
                    x,
                    y,
                    flakes[particle as usize % flakes.len()],
                    colors[particle as usize % colors.len()],
                );
            }
        }
        Effect::Speed => {
            for row in 0..4 {
                let head = (tick * 3 + row * 7) % (width / 2 + 8);
                for tail in 0..6 {
                    for side in [-1, 1] {
                        paint(
                            width / 2 + side * (head - tail),
                            row,
                            if tail == 0 { "⠿" } else { "⠤" },
                            if tail == 0 { Color::White } else { cue.color },
                        );
                    }
                }
            }
        }
        Effect::Whistle => {
            for row in 0..4 {
                for x in 0..width {
                    if (x + row.min(3 - row) + tick) % 7 < 2 {
                        paint(x, row, "⢕", cue.color);
                    }
                }
            }
        }
        Effect::Mirror => {
            for x in 0..width {
                let crest = (elapsed * 5.0 + f64::from(x) * 0.4).sin();
                paint(
                    x,
                    if crest > 0.0 { 2 } else { 3 },
                    if crest > 0.0 { "⠉" } else { "⣀" },
                    cue.color,
                );
            }
        }
        Effect::Fireworks => {
            for burst in 0..3 {
                let age = elapsed - f64::from(burst) * 0.3;
                if age < 0.0 {
                    continue;
                }
                let radius = age % 1.1;
                for ray in 0..12 {
                    let (sin, cos) = (f64::from(ray) * std::f64::consts::TAU / 12.0).sin_cos();
                    let x = f64::from(width * (burst + 1)) / 4.0
                        + radius * f64::from(width) / 3.0 * cos;
                    let y = 1.5 + radius * 3.0 * sin;
                    paint(
                        x.round() as i32,
                        y.round() as i32,
                        "⠳",
                        colors[burst as usize],
                    );
                }
            }
        }
    }
}

fn spectrum(phase: f64) -> Color {
    let channel = |offset: f64| (160.0 + 80.0 * (phase + offset).sin()) as u8;
    Color::Rgb(channel(0.0), channel(2.1), channel(4.2))
}

fn draw_letters(frame: &mut Frame, area: Rect, cue: &Cue, elapsed: f64, mirror: bool) {
    let width = usize::from(area.width);
    let glyph_width = if matches!(cue.effect, Effect::Clock) {
        ((width * 2 / cue.title.len().max(1))
            .saturating_sub(1)
            .clamp(1, 9)
            - 1)
            / 2
            * 2
            + 1
    } else {
        9
    };
    let advance = glyph_width + 1;
    let glyph_height = (usize::from(area.height) * 4 - 1).min(11);
    let repeated = matches!(cue.effect, Effect::Ticker).then(|| {
        format!("{} ", cue.title).repeat(width * 2 / ((cue.title.len() + 1) * advance) + 2)
    });
    let title = repeated.as_deref().unwrap_or(&cue.title);
    let text_width = title.len() * advance;
    let overflow = text_width.saturating_sub(width * 2);
    let scroll = (((elapsed - 0.7) / 1.3).clamp(0.0, 1.0) * overflow as f64) as i32;
    let entrance = (1.0 - elapsed / 0.45).max(0.0);
    let motion = match cue.effect {
        Effect::Speed => -(entrance * width as f64 * 2.0) as i32,
        Effect::Whistle if elapsed < 0.45 => ((elapsed * 24.0).sin() * 2.0) as i32,
        _ => 0,
    };
    let origin = if matches!(cue.effect, Effect::Ticker) {
        -((elapsed * 32.0) % ((cue.title.len() + 1) * advance) as f64) as i32
    } else if matches!(cue.effect, Effect::Ricochet) {
        let leg = (elapsed * 0.9) % 2.0;
        let bounce = if leg <= 1.0 { leg } else { 2.0 - leg };
        if overflow > 0 {
            -(bounce * overflow as f64) as i32
        } else {
            (bounce * (width * 2 - text_width) as f64) as i32
        }
    } else {
        (width * 2).saturating_sub(text_width) as i32 / 2 - scroll + motion
    };
    let split = if matches!(cue.effect, Effect::Split) {
        (entrance * width as f64) as i32
    } else {
        0
    };
    let mut dots = vec![0_u8; width * usize::from(area.height)];
    let bits = [[1, 8], [2, 16], [4, 32], [64, 128]];
    for (letter, mut byte) in title.bytes().enumerate() {
        if matches!(cue.effect, Effect::Scramble)
            && byte != b' '
            && elapsed < (letter + 1) as f64 / title.len() as f64 * 1.4
        {
            let alphabet = b"ABCDEFGHIKLMNOPRSTUXY";
            byte = alphabet[(letter * 7 + (elapsed * 18.0) as usize * 11) % alphabet.len()];
        }
        for (row, mask) in glyph(byte).into_iter().enumerate() {
            for column in 0..5 {
                if mask & (1 << (4 - column)) == 0 {
                    continue;
                }
                for gx in column * glyph_width / 5..((column + 1) * glyph_width).div_ceil(5) {
                    for gy in row * glyph_height / 7..((row + 1) * glyph_height).div_ceil(7) {
                        let local_x = letter * advance + gx;
                        let side = if local_x < text_width / 2 { -1 } else { 1 };
                        let ripple = if mirror {
                            ((elapsed * 8.0 + local_x as f64 * 0.4).sin() * 2.0) as i32
                        } else {
                            0
                        };
                        let burst = if matches!(cue.effect, Effect::Fireworks) {
                            ((text_width as f64 / 2.0 - local_x as f64) * entrance) as i32
                        } else {
                            0
                        };
                        let progress = (elapsed / 0.7).min(1.0);
                        let mut x = origin + local_x as i32 + side * split + ripple + burst;
                        let mut y = if mirror { glyph_height - 1 - gy } else { gy } as i32;
                        match cue.effect {
                            Effect::Concede => y -= ((1.0 - elapsed / 0.6).max(0.0) * 14.0) as i32,
                            Effect::Launch => y += ((1.0 - elapsed / 0.7).max(0.0) * 14.0) as i32,
                            Effect::Gate
                                if (gy as f64 - glyph_height as f64 / 2.0).abs()
                                    > progress * glyph_height as f64 / 2.0 =>
                            {
                                continue;
                            }
                            Effect::Scan if local_x as f64 > progress * text_width as f64 => {
                                continue;
                            }
                            Effect::Laser
                                if (local_x as f64 - text_width as f64 / 2.0).abs()
                                    > progress * text_width as f64 / 2.0 =>
                            {
                                continue;
                            }
                            Effect::Target => {
                                x += ((local_x as f64 - text_width as f64 / 2.0) * entrance * 1.4)
                                    as i32
                            }
                            Effect::Freeze => {
                                x += (((1.0 - elapsed / 0.4).max(0.0) * 12.0) as i32)
                                    * if gy / 2 % 2 == 0 { -1 } else { 1 }
                            }
                            Effect::Flame => {
                                x += ((gy as f64 * 0.8 - elapsed * 5.0).sin() * 1.4) as i32
                            }
                            Effect::Aurora => {
                                y += ((local_x as f64 * 0.2 - elapsed * 3.0).sin() * 1.4) as i32
                            }
                            Effect::Ripple => {
                                let wave = (local_x as f64 * 0.18 - elapsed * 7.0).sin();
                                x += (wave * 2.0) as i32;
                                y += (wave * 1.5) as i32;
                            }
                            Effect::Orbit => x += ((elapsed * 3.0).sin() * 3.0) as i32,
                            Effect::Sparkle
                                if ((local_x * 17 + gy * 11) % 23) as f64
                                    > elapsed / 0.8 * 23.0 =>
                            {
                                continue;
                            }
                            _ => {}
                        }
                        if x >= 0
                            && (x as usize) < width * 2
                            && y >= 0
                            && (y as usize) < usize::from(area.height) * 4
                        {
                            let (x, y) = (x as usize, y as usize);
                            dots[(y / 4) * width + x / 2] |= bits[y % 4][x % 2];
                        }
                    }
                }
            }
        }
    }
    if matches!(cue.effect, Effect::Whistle | Effect::Noise) && elapsed < 1.4 {
        let reveal = if matches!(cue.effect, Effect::Whistle) {
            ((elapsed - 0.55) / 0.65).clamp(0.0, 1.0)
        } else {
            (elapsed / 1.4).min(1.0)
        };
        let dip = if elapsed < 0.55 {
            (elapsed / 0.55 * std::f64::consts::PI).sin()
        } else {
            0.0
        };
        let (tilt_sin, tilt_cos) = (-0.28 * dip).sin_cos();
        for y in 0..usize::from(area.height) * 4 {
            for x in 0..width * 2 {
                if reveal > ((x * 31 + y * 17) % 97) as f64 / 97.0 {
                    continue;
                }
                let on = if matches!(cue.effect, Effect::Noise) {
                    (x * 13 + y * 37 + (elapsed * 14.0) as usize * 11) % 7 < 3
                } else {
                    let dx = x as f64 - (width as f64 - 15.0) - 23.0;
                    let dy = y as f64 - 3.0;
                    let local_x = 23.0 + tilt_cos * dx + tilt_sin * dy;
                    let local_y = 3.0 - tilt_sin * dx + tilt_cos * dy;
                    let (px, py) = (local_x.round() as i32, local_y.round() as i32);
                    let body = if (0..25).contains(&px) && (0..11).contains(&py) {
                        let r = (px - 9).pow(2) + (py - 6).pow(2);
                        (r <= 20 && r > 4)
                            || ((10..24).contains(&px)
                                && (2..5).contains(&py)
                                && !(px < 18 && px > 13 && py < 4))
                    } else {
                        false
                    };
                    let sound = local_x < 6.0
                        && [0.0, 2.5].into_iter().any(|offset| {
                            let radius = 5.5 + (elapsed * 8.0 + offset) % 5.0;
                            (local_y - 6.0).abs() < radius * 0.32
                                && ((local_x - 9.0).hypot(local_y - 6.0) - radius).abs() < 0.65
                        });
                    body || sound
                };
                let cell = &mut dots[y / 4 * width + x / 2];
                if on {
                    *cell |= bits[y % 4][x % 2];
                } else {
                    *cell &= !bits[y % 4][x % 2];
                }
            }
        }
    }
    let sweep = (elapsed * 18.0) as usize % (width + 6);
    for (index, mask) in dots.into_iter().enumerate() {
        let x = index % width;
        if mask == 0 {
            let dot_x = (x * 2) as i32;
            if dot_x >= origin - split - 2 && dot_x < origin + text_width as i32 + split + 2 {
                frame.buffer_mut()[(area.x + x as u16, area.y + (index / width) as u16)]
                    .set_symbol(" ");
            }
            continue;
        }
        let color = if mirror || cue.fading(elapsed) {
            Color::DarkGray
        } else if matches!(cue.effect, Effect::Flame) {
            let heat =
                ((elapsed * 5.0 - x as f64 * 0.7 + (index / width) as f64).sin() + 1.0) / 2.0;
            Color::Rgb(255, (80.0 + heat * 170.0) as u8, (heat * heat * 80.0) as u8)
        } else if matches!(cue.effect, Effect::Noise) && elapsed < 1.4 {
            let clarity = elapsed / 1.4;
            Color::Rgb(
                70,
                (70.0 + clarity * 160.0) as u8,
                (70.0 + clarity * 70.0) as u8,
            )
        } else if matches!(cue.effect, Effect::Scramble) {
            if elapsed < 1.4 {
                Color::LightCyan
            } else {
                cue.color
            }
        } else if matches!(
            cue.effect,
            Effect::Ripple | Effect::Aurora | Effect::Fireworks | Effect::Orbit
        ) {
            spectrum(x as f64 * 0.25 - elapsed * 2.0)
        } else if matches!(cue.effect, Effect::Spotlight) {
            let center = width as f64 * (0.5 + 0.4 * (elapsed * 1.8).sin());
            match (x as f64 - center).abs() as usize {
                0..=2 => Color::White,
                3..=6 => Color::LightMagenta,
                _ => Color::Rgb(140, 80, 190),
            }
        } else if matches!(cue.effect, Effect::Scan | Effect::Sparkle) {
            let glow = (elapsed * 2.0 - x as f64 * 0.25).sin();
            Color::Rgb(
                (100.0 + 90.0 * glow) as u8,
                230,
                (160.0 + 75.0 * glow) as u8,
            )
        } else if !matches!(cue.effect, Effect::Clock) && x.abs_diff(sweep) < 2 {
            Color::White
        } else {
            cue.color
        };
        let symbol = char::from_u32(0x2800 + u32::from(mask)).unwrap();
        frame.buffer_mut()[(area.x + x as u16, area.y + (index / width) as u16)]
            .set_char(symbol)
            .set_style(Style::default().fg(color).add_modifier(Modifier::BOLD));
    }
}

fn glyph(letter: u8) -> [u8; 7] {
    match letter {
        b'A' => [14, 17, 17, 31, 17, 17, 17],
        b'B' => [30, 17, 17, 30, 17, 17, 30],
        b'C' => [14, 17, 16, 16, 16, 17, 14],
        b'D' => [30, 17, 17, 17, 17, 17, 30],
        b'E' => [31, 16, 16, 30, 16, 16, 31],
        b'F' => [31, 16, 16, 30, 16, 16, 16],
        b'G' => [14, 17, 16, 23, 17, 17, 15],
        b'H' => [17, 17, 17, 31, 17, 17, 17],
        b'I' => [31, 4, 4, 4, 4, 4, 31],
        b'K' => [17, 18, 20, 24, 20, 18, 17],
        b'L' => [16, 16, 16, 16, 16, 16, 31],
        b'M' => [17, 27, 21, 21, 17, 17, 17],
        b'N' => [17, 25, 25, 21, 19, 19, 17],
        b'O' => [14, 17, 17, 17, 17, 17, 14],
        b'P' => [30, 17, 17, 30, 16, 16, 16],
        b'R' => [30, 17, 17, 30, 20, 18, 17],
        b'S' => [15, 16, 16, 14, 1, 1, 30],
        b'T' => [31, 4, 4, 4, 4, 4, 4],
        b'U' => [17, 17, 17, 17, 17, 17, 14],
        b'X' => [17, 17, 10, 4, 10, 17, 17],
        b'Y' => [17, 17, 10, 4, 4, 4, 4],
        b'!' => [4, 4, 4, 4, 4, 0, 4],
        b'0' => [14, 17, 17, 17, 17, 17, 14],
        b'1' => [4, 12, 4, 4, 4, 4, 14],
        b'2' => [14, 17, 1, 2, 4, 8, 31],
        b'3' => [30, 1, 1, 14, 1, 1, 30],
        b'4' => [2, 6, 10, 18, 31, 2, 2],
        b'5' => [31, 16, 16, 30, 1, 1, 30],
        b'6' => [14, 16, 16, 30, 17, 17, 14],
        b'7' => [31, 1, 2, 4, 8, 8, 8],
        b'8' => [14, 17, 17, 14, 17, 17, 14],
        b'9' => [14, 17, 17, 15, 1, 1, 14],
        b':' => [0, 4, 4, 0, 4, 4, 0],
        b'.' => [0, 0, 0, 0, 0, 4, 4],
        b'-' => [0, 0, 0, 31, 0, 0, 0],
        b' ' => [0; 7],
        _ => [31, 17, 1, 2, 4, 0, 4],
    }
}

impl Cue {
    pub fn duration(&self) -> Duration {
        match self.effect {
            Effect::Tetris => Duration::from_secs(9),
            Effect::Fluid => Duration::from_secs(6),
            _ => LIFETIME,
        }
    }

    fn fading(&self, elapsed: f64) -> bool {
        elapsed > self.duration().as_secs_f64() - 0.5
    }
}
