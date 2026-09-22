pub const SPEED_OF_LIGHT_KM_S: f64 = 299_792.458;

#[must_use]
pub fn downlink_hz(transmitted_hz: f64, range_rate_km_s: f64) -> f64 {
    transmitted_hz * (1.0 - range_rate_km_s / SPEED_OF_LIGHT_KM_S)
}

#[must_use]
pub fn uplink_hz(wanted_hz: f64, range_rate_km_s: f64) -> f64 {
    wanted_hz / (1.0 - range_rate_km_s / SPEED_OF_LIGHT_KM_S)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_approaching_satellite_is_heard_high() {
        let heard = downlink_hz(437_800_000.0, -7.0);
        assert!((heard - 437_810_222.0).abs() < 1.0, "{heard}");
    }

    #[test]
    fn an_uplink_lands_on_the_wanted_frequency() {
        let sent = uplink_hz(145_990_000.0, 5.5);
        assert!((downlink_hz(sent, 5.5) - 145_990_000.0).abs() < 1e-6);
    }
}
