use num_complex::Complex;
use sdrmm_dsp::{CONFIDENT, Soft};

const CONTINUAL: [usize; 45] = [
    0, 48, 54, 87, 141, 156, 192, 201, 255, 279, 282, 333, 432, 450, 483, 525, 531, 618, 636, 714,
    759, 765, 780, 804, 873, 888, 918, 939, 942, 969, 984, 1050, 1101, 1107, 1110, 1137, 1140,
    1146, 1206, 1269, 1323, 1377, 1491, 1683, 1704,
];
const TPS: [usize; 17] = [
    34, 50, 209, 346, 413, 569, 595, 688, 790, 901, 1073, 1219, 1262, 1286, 1469, 1594, 1687,
];
const SHIFTS: [usize; 6] = [0, 63, 105, 42, 21, 84];

pub struct Mapping {
    pub fft: usize,
    pub carriers: usize,
    pub data: [Vec<usize>; 4],
    pub pilots: [Vec<usize>; 4],
    pub continual: Vec<usize>,
    pub tps: Vec<usize>,
    pub reference: Vec<f32>,
    pub permutation: Vec<usize>,
}

impl Mapping {
    pub fn new(fft: usize) -> Self {
        let copies = fft / 2048;
        let carriers = 1704 * copies + 1;
        let mut continual: Vec<_> = (0..copies)
            .flat_map(|i| CONTINUAL.map(|k| k + i * 1704))
            .collect();
        continual.sort_unstable();
        continual.dedup();
        let tps: Vec<_> = (0..copies)
            .flat_map(|i| TPS.map(|k| k + i * 1704))
            .collect();
        let mut register = 0x7ffu16;
        let reference = (0..carriers)
            .map(|_| {
                let value = if register & 1 == 0 { 1.0 } else { -1.0 };
                register = (register >> 1) | (((register ^ (register >> 2)) & 1) << 10);
                value
            })
            .collect();
        let pilots = std::array::from_fn(|phase| {
            (0..carriers)
                .filter(|k| k % 12 == 3 * phase || continual.binary_search(k).is_ok())
                .collect::<Vec<_>>()
        });
        let data = std::array::from_fn(|phase| {
            (0..carriers)
                .filter(|k| {
                    pilots[phase].binary_search(k).is_err() && tps.binary_search(k).is_err()
                })
                .collect()
        });
        Self {
            fft,
            carriers,
            data,
            pilots,
            continual,
            tps,
            reference,
            permutation: permutation(fft),
        }
    }

    pub fn bin(&self, carrier: usize, offset: isize) -> usize {
        (carrier as isize - (self.carriers / 2) as isize + offset).rem_euclid(self.fft as isize)
            as usize
    }
}

pub fn permutation(fft: usize) -> Vec<usize> {
    let order: &[usize] = if fft == 2048 {
        &[4, 3, 9, 6, 2, 8, 1, 5, 7, 0]
    } else {
        &[7, 1, 4, 2, 9, 6, 8, 10, 0, 3, 11, 5]
    };
    let mut state = 0usize;
    let mut result = Vec::with_capacity(fft * 189 / 256);
    for i in 0..fft {
        state = match i {
            0 | 1 => 0,
            2 => 1,
            _ => {
                let feedback = if fft == 2048 {
                    state ^ (state >> 3)
                } else {
                    state ^ (state >> 1) ^ (state >> 4) ^ (state >> 6)
                };
                (state >> 1) | ((feedback & 1) << (order.len() - 1))
            }
        };
        let mapped = order
            .iter()
            .enumerate()
            .fold((i & 1) * fft / 2, |value, (source, &dest)| {
                value | (((state >> source) & 1) << dest)
            });
        if mapped < fft * 189 / 256 {
            result.push(mapped);
        }
    }
    result
}

pub fn point(word: usize, bits: usize, alpha: usize) -> Complex<f32> {
    let axis = |parity: usize| {
        let sign = if word & (1 << (bits - 1 - parity)) == 0 {
            1.0
        } else {
            -1.0
        };
        let gray = (2 + parity..bits)
            .step_by(2)
            .fold(0usize, |g, bit| (g << 1) | ((word >> (bits - 1 - bit)) & 1));
        let mut binary = gray;
        let mut part = gray >> 1;
        while part != 0 {
            binary ^= part;
            part >>= 1;
        }
        sign * (alpha + 2 * ((1 << (bits / 2 - 1)) - 1 - binary)) as f32
    };
    let levels = 1 << (bits / 2 - 1);
    let power = 2.0
        * (0..levels)
            .map(|i| ((alpha + 2 * i) as f32).powi(2))
            .sum::<f32>()
        / levels as f32;
    Complex::new(axis(0), axis(1)) / power.sqrt()
}

pub fn soften(value: Complex<f32>, table: &[Complex<f32>], bits: usize) -> [Soft; 6] {
    match bits {
        2 => soften_axis::<2>(value, table),
        4 => soften_axis::<4>(value, table),
        _ => soften_axis::<6>(value, table),
    }
}

fn soften_axis<const BITS: usize>(value: Complex<f32>, table: &[Complex<f32>]) -> [Soft; 6] {
    let bits = BITS;
    let mut costs = [[f32::INFINITY; 2]; 6];
    for axis_word in 0..1 << (bits / 2) {
        let word = (0..bits / 2).fold(0, |word, bit| {
            word | (((axis_word >> (bits / 2 - 1 - bit)) & 1) << (bits - 1 - 2 * bit))
        });
        let amplitude = table[word].re;
        let distances = [
            (value.re - amplitude).powi(2),
            (value.im - amplitude).powi(2),
        ];
        for (bit, cost) in costs.iter_mut().take(bits).enumerate() {
            let index = (axis_word >> (bits / 2 - 1 - bit / 2)) & 1;
            cost[index] = cost[index].min(distances[bit % 2]);
        }
    }
    std::array::from_fn(|bit| {
        if bit < bits {
            ((costs[bit][0] - costs[bit][1]) * 2.0 * f32::from(CONFIDENT))
                .clamp(-f32::from(CONFIDENT), f32::from(CONFIDENT)) as Soft
        } else {
            0
        }
    })
}

pub fn deinterleave(
    input: &[[Soft; 6]],
    bits: usize,
    hierarchy: bool,
    low: bool,
    output: &mut Vec<Soft>,
) {
    let lanes: &[usize] = match (bits, hierarchy, low) {
        (_, true, false) => &[0, 1],
        (4, true, true) => &[2, 3],
        (6, true, true) => &[2, 4, 3, 5],
        (2, _, _) => &[0, 1],
        (4, _, _) => &[0, 2, 1, 3],
        _ => &[0, 2, 4, 1, 3, 5],
    };
    output.clear();
    for block in input.as_chunks::<126>().0 {
        for i in 0..126 {
            for &lane in lanes {
                output.push(block[(i + 126 - SHIFTS[lane]) % 126][lane]);
            }
        }
    }
}
