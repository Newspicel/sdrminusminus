pub mod analog;
pub mod constellation;
pub mod cpm;
pub mod linear;
pub mod multicarrier;
pub mod ofdm;
pub mod orthogonal;
pub mod ppm;
pub mod pulse;
pub mod quality;
pub mod soft;
pub mod spread;
pub mod symbolcode;

#[cfg(test)]
#[global_allocator]
static ALLOC: sdrmm_test_support::CountingAlloc = sdrmm_test_support::CountingAlloc::new();
