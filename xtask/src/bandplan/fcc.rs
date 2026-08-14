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
