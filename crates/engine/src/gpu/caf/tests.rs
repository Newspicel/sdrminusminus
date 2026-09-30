use super::*;

fn roomy() -> wgpu::Limits {
    wgpu::Limits {
        max_buffer_size: 1 << 34,
        max_storage_buffer_binding_size: 1 << 34,
        ..wgpu::Limits::default()
    }
}

fn dab() -> BatchShape {
    BatchShape::new(1024, 1000, 548, 2, 103, 4).unwrap()
}

fn groups(batches: usize, per_group: usize, doppler_taps: usize) -> GroupPlan {
    GroupPlan::split(batches, per_group, 0, doppler_taps, true, false).unwrap()
}

#[test]
fn the_dab_default_fits_the_gpu_budget() {
    let plan = groups(1024, 100, 0);
    let footprint = Footprint::of(&dab(), Some(&plan)).unwrap();
    assert_eq!(footprint.spectra, 6 * 1024 * 2048 * 8);
    assert_eq!(footprint.cube, 4 * 548 * 1024 * 8);
    assert_eq!(footprint.window, 5 * dab().window() as u64 * 8);
    assert_eq!(footprint.sums, 10 * (1 + 4) * 2048 * 8);
    assert_eq!(footprint.weights, 10 * 4 * 2048 * 8);
    assert_eq!(footprint.turns, 10 * 1024 * 2 * 8);
    assert_eq!(footprint.fits(&roomy()), Ok(()));
}

#[test]
fn a_plan_over_512_mib_stays_off_the_gpu() {
    let shape = BatchShape::new(4096, 1000, 548, 2, 103, 15).unwrap();
    let footprint = Footprint::of(&shape, None).unwrap();
    assert!(footprint.spectra + footprint.cube > MAX_GPU_BYTES);
    assert!(footprint.fits(&roomy()).is_err());
}

#[test]
fn clutter_sums_count_toward_the_budget() {
    let shape = BatchShape::new(1024, 1000, 548, 2, 103, 15).unwrap();
    let coarse = Footprint::of(&shape, Some(&groups(1024, 100, 2))).unwrap();
    assert_eq!(coarse.fits(&roomy()), Ok(()));
    let fine = Footprint::of(&shape, Some(&groups(1024, 8, 2))).unwrap();
    assert!(fine.spectra + fine.cube < MAX_GPU_BYTES);
    assert!(fine.fits(&roomy()).is_err());
}

#[test]
fn a_buffer_over_the_binding_limit_stays_off_the_gpu() {
    let narrow = wgpu::Limits {
        max_storage_buffer_binding_size: 64 << 20,
        ..roomy()
    };
    assert!(Footprint::of(&dab(), None).unwrap().fits(&narrow).is_err());
}

#[test]
fn a_plan_without_clutter_needs_no_model_slot() {
    let shape = BatchShape::new(512, 260, 73, 0, 0, 4).unwrap();
    let footprint = Footprint::of(&shape, None).unwrap();
    assert_eq!(footprint.spectra, 5 * 512 * shape.fft_len as u64 * 8);
    assert_eq!(
        (footprint.sums, footprint.weights, footprint.turns),
        (0, 0, 0)
    );
    let layout = layout_of(&shape, None).unwrap();
    assert_eq!((layout.first_lane, layout.slots, layout.eca), (1, 5, 0));
}
