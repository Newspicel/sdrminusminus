use num_complex::Complex;
use sdrmm_modem::{
    constellation::Constellation,
    cpm::Mapping,
    quality::{self, Quality},
};
use sdrmm_wire::SymbolPlane;

#[derive(Default)]
pub struct SymbolTap {
    wanted: bool,
    pub plane: Option<SymbolPlane>,
    pub symbols: Vec<f32>,
    pub reference: Vec<f32>,
    pub symbol_rate: f64,
    pub evm: f32,
    pub mer_db: f32,
    pub margin: f32,
    pub freq_error_hz: f32,
}

impl SymbolTap {
    #[must_use]
    pub fn wanted(&self) -> bool {
        self.wanted
    }

    pub fn set_wanted(&mut self, wanted: bool) {
        self.wanted = wanted;
    }

    pub fn clear(&mut self) {
        self.plane = None;
        self.symbols.clear();
        self.reference.clear();
        self.symbol_rate = 0.0;
        self.evm = 0.0;
        self.mer_db = 0.0;
        self.margin = 0.0;
        self.freq_error_hz = 0.0;
    }

    fn record(
        &mut self,
        plane: SymbolPlane,
        symbol_rate: f64,
        freq_error_hz: f64,
        q: Option<Quality>,
    ) {
        self.plane = Some(plane);
        self.symbol_rate = symbol_rate;
        self.freq_error_hz = freq_error_hz as f32;
        let q = q.unwrap_or(Quality {
            evm: 0.0,
            mer_db: 0.0,
            margin: 0.0,
        });
        self.evm = q.evm as f32;
        self.mer_db = capped(q.mer_db);
        self.margin = capped(q.margin);
    }

    pub fn linear(
        &mut self,
        symbols: &[Complex<f32>],
        table: &Constellation,
        symbol_rate: f64,
        freq_error_hz: f64,
    ) {
        if !self.wanted {
            return;
        }
        for point in table.points() {
            self.reference.push(point.re);
            self.reference.push(point.im);
        }
        for symbol in symbols {
            self.symbols.push(symbol.re);
            self.symbols.push(symbol.im);
        }
        let measured = quality::measure_complex(symbols, table);
        self.record(SymbolPlane::Complex, symbol_rate, freq_error_hz, measured);
    }

    pub fn levels(
        &mut self,
        symbols: &[f32],
        carrying: &[bool],
        mapping: &Mapping,
        symbol_rate: f64,
        freq_error_hz: f64,
    ) {
        if !self.wanted {
            return;
        }
        self.reference.extend_from_slice(mapping.levels());
        if carrying.len() == symbols.len() {
            self.symbols.extend(
                symbols
                    .iter()
                    .zip(carrying)
                    .filter_map(|(&s, &live)| live.then_some(s)),
            );
        } else {
            self.symbols.extend_from_slice(symbols);
        }
        let measured = quality::measure_levels(&self.symbols, mapping);
        self.record(SymbolPlane::Level, symbol_rate, freq_error_hz, measured);
    }
}

const READOUT_CEILING: f64 = 99.0;

fn capped(value: f64) -> f32 {
    if value.is_nan() {
        return 0.0;
    }
    value.min(READOUT_CEILING) as f32
}

#[cfg(test)]
mod tests {
    use sdrmm_modem::constellation::tables;

    use super::*;

    #[test]
    fn a_tap_nobody_watches_costs_nothing() {
        let table = tables::psk(4).expect("qpsk");
        let mut tap = SymbolTap::default();
        tap.linear(table.points(), &table, 31.25, 0.0);
        assert!(tap.symbols.is_empty());
        assert!(tap.reference.is_empty());
        assert_eq!(tap.plane, None);
    }

    #[test]
    fn a_watched_linear_tap_carries_the_cloud_its_reference_and_its_measurement() {
        let table = tables::psk(4).expect("qpsk");
        let mut tap = SymbolTap::default();
        tap.set_wanted(true);
        tap.linear(table.points(), &table, 31.25, -4.0);

        assert_eq!(tap.plane, Some(SymbolPlane::Complex));
        assert_eq!(tap.symbols.len(), table.len() * 2);
        assert_eq!(tap.reference.len(), table.len() * 2);
        assert_eq!(tap.symbol_rate, 31.25);
        assert_eq!(tap.freq_error_hz, -4.0);
        assert!(tap.evm < 1e-5, "a clean cloud measured {} EVM", tap.evm);
        assert_eq!(tap.margin, READOUT_CEILING as f32);
        assert_eq!(tap.mer_db, READOUT_CEILING as f32);
    }

    #[test]
    fn a_watched_level_tap_carries_its_own_rail() {
        let mapping = Mapping::natural(4);
        let sent: Vec<f32> = (0..64).map(|i| mapping.level((i % 4) as u8)).collect();
        let mut tap = SymbolTap::default();
        tap.set_wanted(true);
        tap.levels(&sent, &[], &mapping, 4800.0, 12.0);

        assert_eq!(tap.plane, Some(SymbolPlane::Level));
        assert_eq!(tap.symbols.len(), sent.len());
        assert_eq!(tap.reference.len(), mapping.m());
        assert_eq!(tap.symbol_rate, 4800.0);
        assert!(tap.evm < 1e-5, "a clean rail measured {} EVM", tap.evm);
    }

    #[test]
    fn a_clear_leaves_the_tap_watched_so_the_next_block_still_fills_it() {
        let mapping = Mapping::natural(4);
        let mut tap = SymbolTap::default();
        tap.set_wanted(true);
        tap.levels(&[1.0, -1.0], &[], &mapping, 4800.0, 0.0);
        tap.clear();

        assert!(tap.wanted());
        assert!(tap.symbols.is_empty());
        assert_eq!(tap.plane, None);
        assert_eq!(tap.symbol_rate, 0.0);
    }

    #[test]
    fn an_unmeasurable_block_reports_zero_rather_than_an_infinity() {
        let table = tables::psk(4).expect("qpsk");
        let mut tap = SymbolTap::default();
        tap.set_wanted(true);
        tap.linear(&[], &table, 31.25, 0.0);

        assert_eq!(tap.plane, Some(SymbolPlane::Complex));
        assert!(tap.symbols.is_empty());
        assert_eq!(tap.mer_db, 0.0);
        assert_eq!(tap.margin, 0.0);
        assert_eq!(tap.evm, 0.0);
    }
}
