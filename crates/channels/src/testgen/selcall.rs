use num_complex::Complex;
use sdrmm_wire::SelcallSystem;

use super::{fm_modulate, silence};

const DEVIATION_HZ: f64 = 2_500.0;

pub fn transmission(
    system: SelcallSystem,
    code: &str,
    rate: f64,
) -> Result<Vec<Complex<f32>>, &'static str> {
    let tone_ms = match system {
        SelcallSystem::Ccir1 => 100,
        SelcallSystem::Zvei1 => 70,
    };
    let samples_per_tone = (rate * f64::from(tone_ms) / 1_000.0) as usize;
    let mut audio = vec![0.0; (rate * 0.1) as usize];
    let mut previous_on_air = None;
    for symbol in code.chars() {
        let on_air = if previous_on_air == Some(symbol) {
            'R'
        } else {
            symbol
        };
        let hz = frequency(system, on_air).ok_or("call contains a symbol outside the tone plan")?;
        let start = audio.len();
        audio.resize(start + samples_per_tone, 0.0);
        for (index, sample) in audio[start..].iter_mut().enumerate() {
            *sample = 0.8 * (std::f64::consts::TAU * hz * index as f64 / rate).sin() as f32;
        }
        previous_on_air = Some(on_air);
    }
    let mut iq = fm_modulate(&audio, DEVIATION_HZ, rate);
    iq.extend(silence((rate * 0.2) as usize));
    Ok(iq)
}

fn frequency(system: SelcallSystem, symbol: char) -> Option<f64> {
    let table: &[(char, f64)] = match system {
        SelcallSystem::Ccir1 => &[
            ('0', 1_981.0),
            ('1', 1_124.0),
            ('2', 1_197.0),
            ('3', 1_275.0),
            ('4', 1_358.0),
            ('5', 1_446.0),
            ('6', 1_540.0),
            ('7', 1_640.0),
            ('8', 1_747.0),
            ('9', 1_860.0),
            ('R', 2_110.0),
        ],
        SelcallSystem::Zvei1 => &[
            ('0', 2_400.0),
            ('1', 1_060.0),
            ('2', 1_160.0),
            ('3', 1_270.0),
            ('4', 1_400.0),
            ('5', 1_530.0),
            ('6', 1_670.0),
            ('7', 1_830.0),
            ('8', 2_000.0),
            ('9', 2_200.0),
            ('A', 2_800.0),
            ('B', 810.0),
            ('C', 970.0),
            ('D', 885.0),
            ('R', 2_600.0),
        ],
    };
    table
        .iter()
        .find_map(|&(name, hz)| (name == symbol).then_some(hz))
}
