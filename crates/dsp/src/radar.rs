pub mod aoa;
pub mod assign;
pub mod batch;
pub mod bistatic;
pub mod cfar;
pub mod cluster;
pub mod cma;
pub mod nlms;
pub mod threshold;
pub mod track;
pub mod wiener;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RadarDspError {
    #[error("radar batch shape is invalid")]
    Shape,
    #[error("clutter filter order is too large")]
    Order,
    #[error("clutter covariance is singular")]
    Singular,
    #[error("radar FFT size is out of range")]
    Size,
}
