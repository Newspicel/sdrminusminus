//! The FCC Online Table of Frequency Allocations — 47 CFR §2.106.
//!
//! Four layers from one document. §2.106 reproduces the ITU table's Region 1, 2 and 3 columns
//! beside the US Federal and Non-Federal ones, so this is also where the ITU layers come from:
//! the International Table *is* the ITU table as codified, and the FCC publishes it where the
//! ITU sells it.
//!
//! Source: <https://transition.fcc.gov/oet/spectrum/table/fcctable.pdf>.

use anyhow::{Result, bail};

use super::{Row, Target};

// Named now because they are the point of this source: one document, four layers. Unreachable
// until `parse` lands, which is what the module comment is about.
#[expect(
    dead_code,
    reason = "targets for the importer described in the module comment"
)]
pub(super) static ITU_R1: &Target = &Target {
    id: "itu-r1",
    name: "ITU Region 1",
    authority: "ITU",
    kind: "world",
};
#[expect(dead_code, reason = "see ITU_R1")]
pub(super) static ITU_R2: &Target = &Target {
    id: "itu-r2",
    name: "ITU Region 2",
    authority: "ITU",
    kind: "world",
};
#[expect(dead_code, reason = "see ITU_R1")]
pub(super) static ITU_R3: &Target = &Target {
    id: "itu-r3",
    name: "ITU Region 3",
    authority: "ITU",
    kind: "world",
};
#[expect(dead_code, reason = "see ITU_R1")]
pub(super) static FCC: &Target = &Target {
    id: "us",
    name: "United States — FCC",
    authority: "FCC",
    kind: "regulatory",
};

pub(super) fn parse(_input: &str) -> Result<Vec<(&'static Target, Vec<Row>)>> {
    bail!(
        "the FCC importer needs word coordinates, not `pdftotext -layout` output — see the \
         module comment in xtask/src/bandplan/fcc.rs"
    )
}
