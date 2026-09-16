use std::path::Path;

use anyhow::Result;
use sdrmm_channels::testgen;
use sdrmm_recorder::SigmfWriter;
use sdrmm_wire::DabTransmissionMode;

pub fn run(out: &Path) -> Result<()> {
    std::fs::create_dir_all(out)?;
    for (name, rate, iq) in [
        (
            "broadcast-dab",
            2_048_000.0,
            testgen::dab::ensemble_with_data(DabTransmissionMode::I, 48),
        ),
        (
            "broadcast-dvbt",
            64_000_000.0 / 7.0,
            testgen::dvbt::waveform(testgen::dvbt::defaults(), 2176),
        ),
        (
            "broadcast-dvbs2sf",
            2_000_000.0,
            testgen::datv::dvbs2_superframes(4),
        ),
        ("broadcast-dvbs", 2_000_000.0, testgen::datv::dvbs(4)),
        ("broadcast-dvbs2", 2_000_000.0, testgen::datv::dvbs2(4)),
        (
            "broadcast-dvbs2x",
            2_000_000.0,
            testgen::datv::dvbs2_mode(
                4,
                sdrmm_channels::Dvbs2Modulation::Apsk256,
                sdrmm_channels::Dvbs2Rate::R135_180,
                false,
                true,
            ),
        ),
    ] {
        let stem = out.join(name);
        let mut writer =
            SigmfWriter::create(&stem, rate, 220_352_000.0, "Synthetic broadcast media")?;
        writer.write_block(&iq)?;
        writer.finalize()?;
        println!("{}: {} IQ samples at {rate} Hz", stem.display(), iq.len());
    }
    Ok(())
}
