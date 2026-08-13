# Third-party notices

Release installers and container images include SoapySDR and a curated set of hardware modules.
Their package metadata and license texts are shipped under `soapy/licenses` in desktop bundles.

- SoapySDR: Boost Software License 1.0
- rust-soapysdr: Apache-2.0 OR Boost Software License 1.0
- SoapyRTLSDR and rtl-sdr: Boost Software License 1.0 / GPL-2.0-or-later
- SoapyHackRF (`soapysdr-module-hackrf`): MIT
- HackRF runtime (`libhackrf0`): GPL-2.0-or-later
- HackRF public API declarations (`hackrf.h`): BSD-3-Clause
- Airspy, AirspyHF, bladeRF, LimeSuite, libiio/PlutoSDR, and SoapyRemote: see the bundled package
  metadata and license files for the exact versions in each platform package.

UHD and SDRplay are not part of the base package. Their modules may be installed as optional
packs when their size and redistribution terms are acceptable.
