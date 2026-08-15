use sdrmm_wire::{
    AntennaGeometry, AntennaPoint, AntennaSegment, AntennaSegmentRole as Role, MAX_RADIAL_SLOPE_DEG,
};

const COIL_HEIGHT_FRACTION: f64 = 0.05;

fn at(x_m: f64, y_m: f64, z_m: f64) -> AntennaPoint {
    AntennaPoint::new(x_m, y_m, z_m)
}

fn segment(label: &str, role: Role, from: AntennaPoint, to: AntennaPoint) -> AntennaSegment {
    AntennaSegment {
        label: label.to_owned(),
        role,
        from,
        to,
    }
}

fn radial_fan(base: AntennaPoint, length_m: f64, count: u8, slope_deg: f64) -> Vec<AntennaSegment> {
    let slope = slope_deg.clamp(0.0, MAX_RADIAL_SLOPE_DEG).to_radians();
    let reach = length_m * slope.cos();
    let drop = length_m * slope.sin();
    (0..count)
        .map(|index| {
            let bearing = std::f64::consts::TAU * f64::from(index) / f64::from(count.max(1));
            segment(
                "Radial",
                Role::Radial,
                base,
                at(
                    base.x_m + reach * bearing.cos(),
                    base.y_m - drop,
                    base.z_m + reach * bearing.sin(),
                ),
            )
        })
        .collect()
}

pub(super) fn dipole(leg_m: f64) -> AntennaGeometry {
    AntennaGeometry {
        segments: vec![
            segment(
                "Leg",
                Role::Driven,
                at(-leg_m, 0.0, 0.0),
                AntennaPoint::ORIGIN,
            ),
            segment(
                "Leg",
                Role::Driven,
                AntennaPoint::ORIGIN,
                at(leg_m, 0.0, 0.0),
            ),
        ],
        feed: AntennaPoint::ORIGIN,
    }
}

pub(super) fn inverted_v(leg_m: f64, half_apex_rad: f64) -> AntennaGeometry {
    let apex = at(0.0, leg_m * half_apex_rad.cos(), 0.0);
    let reach = leg_m * half_apex_rad.sin();
    AntennaGeometry {
        segments: vec![
            segment("Leg", Role::Driven, apex, at(-reach, 0.0, 0.0)),
            segment("Leg", Role::Driven, apex, at(reach, 0.0, 0.0)),
        ],
        feed: apex,
    }
}

pub(super) fn ground_plane(
    radiator_m: f64,
    radial_m: f64,
    radials: u8,
    slope_deg: f64,
) -> AntennaGeometry {
    let mut segments = vec![segment(
        "Radiator",
        Role::Driven,
        AntennaPoint::ORIGIN,
        at(0.0, radiator_m, 0.0),
    )];
    segments.extend(radial_fan(
        AntennaPoint::ORIGIN,
        radial_m,
        radials,
        slope_deg,
    ));
    AntennaGeometry {
        segments,
        feed: AntennaPoint::ORIGIN,
    }
}

pub(super) fn five_eighths_vertical(
    radiator_m: f64,
    radial_m: f64,
    radials: u8,
) -> AntennaGeometry {
    let coil_top = at(0.0, radiator_m * COIL_HEIGHT_FRACTION, 0.0);
    let mut segments = vec![
        segment(
            "Base loading coil",
            Role::Matching,
            AntennaPoint::ORIGIN,
            coil_top,
        ),
        segment(
            "Radiator",
            Role::Driven,
            coil_top,
            at(0.0, coil_top.y_m + radiator_m, 0.0),
        ),
    ];
    segments.extend(radial_fan(AntennaPoint::ORIGIN, radial_m, radials, 0.0));
    AntennaGeometry {
        segments,
        feed: AntennaPoint::ORIGIN,
    }
}

pub(super) fn folded_dipole(conductor_m: f64, spacing_m: f64) -> AntennaGeometry {
    let half = conductor_m / 2.0;
    AntennaGeometry {
        segments: vec![
            segment(
                "Conductor",
                Role::Driven,
                at(-half, 0.0, 0.0),
                at(half, 0.0, 0.0),
            ),
            segment(
                "Conductor",
                Role::Driven,
                at(-half, spacing_m, 0.0),
                at(half, spacing_m, 0.0),
            ),
            segment(
                "End spacing",
                Role::Driven,
                at(-half, 0.0, 0.0),
                at(-half, spacing_m, 0.0),
            ),
            segment(
                "End spacing",
                Role::Driven,
                at(half, 0.0, 0.0),
                at(half, spacing_m, 0.0),
            ),
        ],
        feed: AntennaPoint::ORIGIN,
    }
}

pub(super) fn j_pole(
    radiator_m: f64,
    stub_m: f64,
    spacing_m: f64,
    feed_height_m: f64,
) -> AntennaGeometry {
    AntennaGeometry {
        segments: vec![
            segment(
                "Radiator (long element)",
                Role::Driven,
                AntennaPoint::ORIGIN,
                at(0.0, radiator_m, 0.0),
            ),
            segment(
                "Matching stub (short element)",
                Role::Matching,
                at(spacing_m, 0.0, 0.0),
                at(spacing_m, stub_m, 0.0),
            ),
            segment(
                "Element spacing",
                Role::Matching,
                AntennaPoint::ORIGIN,
                at(spacing_m, 0.0, 0.0),
            ),
        ],
        feed: at(spacing_m / 2.0, feed_height_m, 0.0),
    }
}

pub(super) struct BoomElement {
    pub name: String,
    pub length_m: f64,
    pub position_m: f64,
    pub driven: bool,
}

pub(super) fn yagi(elements: &[BoomElement], boom_m: f64) -> AntennaGeometry {
    let mut segments = vec![segment(
        "Boom",
        Role::Structure,
        AntennaPoint::ORIGIN,
        at(0.0, 0.0, boom_m),
    )];
    let mut feed = AntennaPoint::ORIGIN;
    for element in elements {
        let half = element.length_m / 2.0;
        if element.driven {
            feed = at(0.0, 0.0, element.position_m);
        }
        segments.push(segment(
            &element.name,
            if element.driven {
                Role::Driven
            } else {
                Role::Parasitic
            },
            at(-half, 0.0, element.position_m),
            at(half, 0.0, element.position_m),
        ));
    }
    AntennaGeometry { segments, feed }
}

pub(super) fn quad_loop(side_m: f64, matching_line_m: f64) -> AntennaGeometry {
    let half = side_m / 2.0;
    let corners = [
        at(-half, 0.0, 0.0),
        at(half, 0.0, 0.0),
        at(half, side_m, 0.0),
        at(-half, side_m, 0.0),
    ];
    let mut segments: Vec<AntennaSegment> = (0..corners.len())
        .map(|index| {
            segment(
                "Side",
                Role::Driven,
                corners[index],
                corners[(index + 1) % corners.len()],
            )
        })
        .collect();
    segments.push(segment(
        "Quarter-wave 75 Ω matching line",
        Role::Feedline,
        AntennaPoint::ORIGIN,
        at(0.0, -matching_line_m, 0.0),
    ));
    AntennaGeometry {
        segments,
        feed: AntennaPoint::ORIGIN,
    }
}

pub(super) fn end_fed_half_wave(radiator_m: f64, counterpoise_m: f64) -> AntennaGeometry {
    AntennaGeometry {
        segments: vec![
            segment(
                "Radiator",
                Role::Driven,
                AntennaPoint::ORIGIN,
                at(radiator_m, 0.0, 0.0),
            ),
            segment(
                "Counterpoise",
                Role::Radial,
                AntennaPoint::ORIGIN,
                at(-counterpoise_m, 0.0, 0.0),
            ),
        ],
        feed: AntennaPoint::ORIGIN,
    }
}
