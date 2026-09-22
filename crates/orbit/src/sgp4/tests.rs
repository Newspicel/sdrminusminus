use super::*;

const SETS: [(&str, &str, &str); 8] = [
    (
        "near earth",
        "1 25544U 98067A   24001.50000000  .00016717  00000-0  30306-3 0  9999",
        "2 25544  51.6416 247.4627 0006703 130.5360 325.0288 15.50377579432041",
    ),
    (
        "eccentric",
        "1 00005U 58002B   00179.78495062  .00000023  00000-0  28098-4 0  4753",
        "2 00005  34.2682 348.7242 1859667 331.7664  19.3264 10.82419157413667",
    ),
    (
        "polar",
        "1 28057U 03049A   06177.78615833  .00000060  00000-0  35940-4 0  1634",
        "2 28057  98.4283 247.6961 0000884  88.1964 271.9322 14.35478047151241",
    ),
    (
        "low perigee",
        "1 99999U 24001A   24001.50000000  .00050000  00000-0  50000-3 0  9997",
        "2 99999  51.6000 100.0000 0010000 100.0000 260.0000 16.20000000000010",
    ),
    (
        "half day resonance",
        "1 08195U 75081A   06176.33215444  .00000099  00000-0  11873-3 0   813",
        "2 08195  64.1586 279.0717 6877146 264.7651  20.2257  2.00491383225656",
    ),
    (
        "geostationary",
        "1 43700U 18090A   24001.50000000  .00000100  00000-0  00000-0 0  9995",
        "2 43700   0.0543  87.2931 0001822 312.7102 147.3500  1.00271332000017",
    ),
    (
        "inclined geosynchronous",
        "1 23839U 96020A   06176.84348714 -.00000038  00000-0  10000-3 0  9997",
        "2 23839   2.4515  66.0547 0003581  24.7520 145.9212  1.00271094040023",
    ),
    (
        "deep without resonance",
        "1 20413U 83086B   24001.25000000 -.00000024  00000-0  00000-0 0  9996",
        "2 20413  55.1100  43.0180 0137640  40.3810 320.6590  2.00562362265304",
    ),
];

const MINUTES: [f64; 9] = [
    0.0, 1.0, 45.0, 360.0, 720.0, 1_440.0, 4_320.0, 10_080.0, -1_440.0,
];

fn ours(line1: &str, line2: &str) -> Propagator {
    let tle = Tle::parse(&format!("{line1}\n{line2}")).expect("valid set");
    Propagator::new(&tle).expect("usable elements")
}

fn reference(line1: &str, line2: &str) -> sgp4::Constants {
    let elements = sgp4::Elements::from_tle(None, line1.as_bytes(), line2.as_bytes())
        .expect("reference parses");
    sgp4::Constants::from_elements(&elements).expect("reference initialises")
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f64>().sqrt()
}

#[test]
fn every_orbit_class_matches_the_reference_propagator() {
    for (name, line1, line2) in SETS {
        let ours = ours(line1, line2);
        let reference = reference(line1, line2);
        for minutes in MINUTES {
            let expected = reference.propagate(sgp4::MinutesSinceEpoch(minutes));
            let got = ours.propagate(minutes);
            let (Ok(expected), Ok(got)) = (expected, got) else {
                panic!("{name} at {minutes} min: one side failed");
            };
            let position = distance(got.position_km, expected.position);
            let velocity = distance(got.velocity_km_s, expected.velocity);
            let days = minutes.abs() / 1_440.0;
            let allowed_km = 0.1 + 0.4 * days * days;
            assert!(
                position < allowed_km,
                "{name} at {minutes} min: {position} km off"
            );
            assert!(
                velocity < allowed_km / 1_000.0,
                "{name} at {minutes} min: {velocity} km/s off"
            );
        }
    }
}

#[test]
fn a_decayed_orbit_says_so() {
    let (_, line1, line2) = SETS[3];
    let propagator = ours(line1, line2);
    assert!(propagator.propagate(60.0 * 1_440.0).is_err());
}

#[test]
fn vallado_verification_values_are_reproduced() {
    let (_, line1, line2) = SETS[1];
    let propagator = ours(line1, line2);
    let expected = [
        (
            0.0,
            [7_022.465_292_66, -1_400.082_967_55, 0.039_951_55],
            [1.893_841_015, 6.405_893_759, 4.534_807_250],
        ),
        (
            360.0,
            [-7_154.031_202_02, -3_783.176_825_04, -3_536.194_122_94],
            [4.741_887_409, -4.151_817_765, -2.093_935_425],
        ),
    ];
    for (minutes, position, velocity) in expected {
        let state = propagator.propagate(minutes).expect("propagates");
        assert!(
            distance(state.position_km, position) < 1e-6,
            "{minutes}: {state:?}"
        );
        assert!(
            distance(state.velocity_km_s, velocity) < 1e-8,
            "{minutes}: {state:?}"
        );
    }
}
