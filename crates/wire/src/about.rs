//! What sdr-- is, and what it is built out of.
//!
//! sdr-- is MIT licensed, and almost everything it links is MIT or Apache-2.0 — but "almost"
//! is not "all", and permissive is not the same as unconditional. MIT and BSD both require the
//! copyright notice to travel with the binary, so a release that ships the code and not the
//! notices is out of compliance no matter how liberal the licenses are. That is what these
//! types carry: the attribution the distributed artifact owes, in the artifact itself, rather
//! than in a file somebody has to go and find.
//!
//! The data behind them is *generated* (`cargo xtask licenses`) from the same lockfiles the
//! build resolves, so the list describes the build that is running rather than the build
//! somebody last remembered to write down.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Which part of the product a component arrives through.
///
/// The distinction is about how it reaches the user, not about language: [`Native`] components
/// are shipped as libraries next to the binary and loaded at runtime, so their obligations
/// attach to the installer rather than to the executable.
///
/// [`Native`]: ComponentSource::Native
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ComponentSource {
    /// A crate compiled into the server or desktop binary.
    Rust,
    /// An npm package bundled into the web UI.
    Web,
    /// A shared library shipped alongside the binary — SoapySDR and its hardware modules.
    Native,
}

impl ComponentSource {
    /// Section heading for the generated notices file and the About panel.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Rust => "Rust crates",
            Self::Web => "Web packages",
            Self::Native => "Bundled native libraries",
        }
    }
}

/// One third-party component the release distributes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Attribution {
    pub name: String,
    /// Absent for native libraries, whose version is whatever the platform package pinned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// The SPDX expression the component declares for itself, verbatim. Not normalized:
    /// `MIT/Apache-2.0` and `MIT OR Apache-2.0` are the same offer, but only one of them is
    /// what the package actually says, and the notice should say what the package says.
    pub license: String,
    pub source: ComponentSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Ids of the license texts this component ships, resolvable through
    /// `GET /api/about/licenses/{id}`. Empty when the component declares an SPDX expression but
    /// publishes no license file of its own — common for crates that rely on the SPDX id alone.
    ///
    /// Always serialized, empty included: a client that has to tell "no texts" apart from "the
    /// field was omitted" is a client that will one day render neither.
    pub texts: Vec<String>,
    /// Set only where the SPDX id is not the whole story: a copyleft relink offer, a patent
    /// encumbrance, a library that is loaded but never linked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// `GET /api/about` — the running build and everything it owes attribution to.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AboutResponse {
    pub name: String,
    /// `CARGO_PKG_VERSION` of the running server, not of the generated notices: the notices are
    /// committed and the version is stamped at release, so only one of the two can be trusted
    /// to say which build this is.
    pub version: String,
    /// SPDX id of sdr-- itself.
    pub license: String,
    /// The project's own `LICENSE`, in full.
    pub license_text: String,
    pub repository: String,
    /// Third-party components, ordered by source and then by name.
    pub components: Vec<Attribution>,
}

/// `GET /api/about/licenses/{id}` — one license text, addressed by content.
///
/// Texts are addressed by a hash of themselves because that is what deduplicates them: several
/// hundred crates offer Apache-2.0 and ship byte-identical copies of it, while every MIT copy
/// differs in its copyright line and none may be collapsed into another. Content addressing
/// keeps the first case cheap without lying about the second.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct LicenseTextResponse {
    pub id: String,
    pub text: String,
}
