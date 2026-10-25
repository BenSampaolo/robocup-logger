use std::{collections::VecDeque, fs, time::Instant};

const WIDTH: usize = 80;
const HEIGHT: usize = 16;
const CELLS: usize = WIDTH * HEIGHT;
const FRAMES: usize = 91;
const STEPS_PER_FRAME: usize = 1000;
const WALL: usize = usize::MAX;

fn lettering() -> Vec<bool> {
    let mut solid = vec![false; CELLS];
    for (letter, ch) in b"KICK IN".iter().enumerate() {
        let rows = match ch {
            b'K' => [17, 18, 20, 24, 20, 18, 17],
            b'I' => [31, 4, 4, 4, 4, 4, 31],
            b'C' => [14, 17, 16, 16, 16, 17, 14],
            b'N' => [17, 25, 25, 21, 19, 19, 17],
            _ => [0; 7],
        };
        for (row, bits) in rows.iter().enumerate() {
            for col in 0_usize..5 {
                if bits & (1 << (4 - col)) == 0 {
                    continue;
                }
                for y in row * 11 / 7..((row + 1) * 11).div_ceil(7) {
                    for x in col * 9 / 5..((col + 1) * 9).div_ceil(5) {
                        solid[(y + 1) * WIDTH + 5 + letter * 10 + x] = true;
                    }
                }
            }
        }
    }
    solid
}

struct Liquid {
    solid: Vec<bool>,
    neighbors: Vec<[usize; 4]>,
    free: Vec<usize>,
    mass: Vec<f64>,
    injected: f64,
    tick: usize,
}

impl Liquid {
    fn new() -> Self {
        let solid = lettering();
        let mut neighbors = vec![[WALL; 4]; CELLS];
        let free: Vec<_> = (0..CELLS).filter(|&i| !solid[i]).collect();
        for &i in &free {
            let (x, y) = (i % WIDTH, i / WIDTH);
            for (direction, (dx, dy)) in [(0, 1), (-1, 0), (1, 0), (0, -1)].iter().enumerate() {
                let (nx, ny) = (x as isize + dx, y as isize + dy);
                if (0..WIDTH as isize).contains(&nx) && (0..HEIGHT as isize).contains(&ny) {
                    let j = ny as usize * WIDTH + nx as usize;
                    if !solid[j] {
                        neighbors[i][direction] = j;
                    }
                }
            }
        }
        let mut seen = vec![false; CELLS];
        let mut queue = VecDeque::from([0]);
        seen[0] = true;
        while let Some(i) = queue.pop_front() {
            for &j in &neighbors[i] {
                if j != WALL && !seen[j] {
                    seen[j] = true;
                    queue.push_back(j);
                }
            }
        }
        assert_eq!(seen.iter().filter(|&&v| v).count(), free.len());
        Self {
            solid,
            neighbors,
            free,
            mass: vec![0.0; CELLS],
            injected: 0.0,
            tick: 0,
        }
    }

    fn step(&mut self) {
        for i in [0, WIDTH, WIDTH * 2, WIDTH * 3] {
            assert!(!self.solid[i]);
            let added = (1.5 - self.mass[i]).clamp(0.0, 0.8);
            self.mass[i] += added;
            self.injected += added;
        }
        let mut next = self.mass.clone();
        for &i in &self.free {
            let mut remaining = self.mass[i];
            if remaining <= 1e-10 {
                continue;
            }
            let mut neighbors = self.neighbors[i];
            if self.tick % 2 != 0 {
                neighbors.swap(1, 2);
            }
            for (direction, j) in neighbors.into_iter().enumerate() {
                if j == WALL {
                    continue;
                }
                let mut flow = match direction {
                    0 => stable_lower(remaining + self.mass[j]) - self.mass[j],
                    3 => remaining - stable_lower(remaining + self.mass[j]),
                    _ => (remaining - self.mass[j]) * 0.25,
                };
                if (direction == 0 || direction == 3) && flow > 0.01 {
                    flow *= 0.5;
                }
                flow = flow.clamp(0.0, remaining.min(1.0));
                next[i] -= flow;
                next[j] += flow;
                remaining -= flow;
            }
        }
        self.mass = next;
        self.tick += 1;
    }

    fn snapshot(&self, output: &mut Vec<u8>) {
        assert!((self.mass.iter().sum::<f64>() - self.injected).abs() < 1e-7);
        for (i, &mass) in self.mass.iter().enumerate() {
            assert!(mass.is_finite() && mass >= -1e-12);
            assert!(!self.solid[i] || mass == 0.0, "water entered a letter");
            output.push((mass.clamp(0.0, 1.0) * 255.0).round() as u8);
        }
    }
}

fn stable_lower(total: f64) -> f64 {
    if total <= 1.0 {
        1.0
    } else if total < 2.02 {
        (1.0 + total * 0.02) / 1.02
    } else {
        (total + 0.02) * 0.5
    }
}

fn main() {
    let start = Instant::now();
    let destination = std::env::args_os()
        .nth(1)
        .expect("expected asset output path");
    let mut liquid = Liquid::new();
    let mut output: Vec<u8> = liquid.solid.iter().map(|&s| u8::from(s)).collect();
    for frame in 0..FRAMES {
        let target = frame * frame * STEPS_PER_FRAME / (FRAMES - 1);
        while liquid.tick < target {
            liquid.step();
        }
        liquid.snapshot(&mut output);
        if frame >= 75 {
            assert!(
                liquid.free.iter().all(|&i| liquid.mass[i] >= 0.999),
                "tank did not finish filling"
            );
        }
    }
    assert_eq!(output.len(), CELLS * (FRAMES + 1));
    assert!(output[CELLS..CELLS * 2].iter().all(|&v| v == 0));
    fs::write(destination, &output).expect("write fluid asset");
    println!(
        "Baked {FRAMES} collision frames, {} bytes, in {:.2}s; volume conserved and solid cells dry.",
        output.len(),
        start.elapsed().as_secs_f64()
    );
}
