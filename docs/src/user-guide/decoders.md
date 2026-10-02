# Decoders

Every mode is a [channel](channels.md). This page lists them, says how well each is tested, and
covers the modes that need more than a frequency.

## Catalog

**+ Add** lists every mode under Decoders, grouped as below.

| Group | Tested on air | Fixture only | Experimental |
|---|---|---|---|
| Analog voice | AM, NFM, SSB, WFM (broadcast) | | |
| Digital voice | DMR, FreeDV 1600 | D-STAR, System Fusion, NXDN, P25 Phase 1, dPMR, M17 | |
| Aviation | ADS-B (1090ES) | ACARS, VDL Mode 2, High Frequency Data Link, [Inmarsat Classic Aero](#inmarsat-and-iridium) | VOR, ILS localizer / glideslope |
| Marine | | AIS, NAVTEX, Digital Selective Calling, Inmarsat STD-C / EGC | |
| Amateur and HF | CW skimmer, FT8, FT4, WSPR | APRS / AX.25 with [weather](#aprs-weather), RTTY, PSK, Morse (CW) | |
| Paging and telemetry | POCSAG | FLEX pager, ERMES pager, Selcall (CCIR/ZVEI), Radio clock (DCF77 / WWVB / MSF / JJY) | |
| Pictures and video | | [SSTV](#sstv), ATV | |
| Weather and satellites | | [NOAA APT, Meteor LRPT](#weather-satellites), [WEFAX](#wefax), [Radiosonde](#radiosondes) | |
| Broadcast digital | [DAB / DAB+](#dab-and-dab) | [DRM30 / DRM+](#drm) | [DVB-T/T2, DATV (DVB-S / S2)](#dvb) |
| Utility | [Signal identifier](scanning.md#identify-a-signal) | [Iridium bursts](#inmarsat-and-iridium), [DECT](#dect) | GNSS lab (GPS L1 C/A) |

| Label | Means |
|---|---|
| Tested on air | Verified live, through a real radio and the full receiver |
| Fixture only | Verified on recordings, generated IQ, or reference vectors, not yet live |
| Experimental | Works with the limits below |

Fixtures catch decoding bugs but say little about drift, fading, or interference. The
[fixture library](https://github.com/Newspicel/sdrmm/blob/main/fixtures/README.md) lists where each
recording came from. VDL Mode 2, HFDL, Inmarsat Classic Aero and STD-C, DSC and Iridium started as
ports of [xng](https://github.com/airframesio/xng).

### Experimental limits

| Mode | Works | Missing |
|---|---|---|
| DVB-T/T2 | DVB-T HP and LP, T2-Base and Lite, SISO and MISO, 1K to 32K, PLP choice, audio, video | GSE, multi-RF TFS |
| GNSS lab | GPS L1 C/A acquisition, tracking and navigation data, one PRN per channel | Position fix |

## DMR trunking

Add **DMR trunk system**, wire Device `iq`, and enter the control channel in MHz under
**Control**. Set **Protocol** or leave it on **Auto-detect**. The node runs the DMR channels it
needs.

| Protocol | Finds its channels by |
|---|---|
| Tier III / Capacity Max | Reading the channel plan from the control channel |
| Capacity Plus | **Search**: carriers that follow the same rest channel |
| Hytera XPT | Same as Capacity Plus |

**Range** narrows the search. Channels can also be entered in the plan table.

Following runs on the server with no browser open. Voice channels must fit inside the Device's
window; a grant outside it is reported. To save call audio, see
[Record each call](recording.md#record-each-call).

## Inmarsat and Iridium

Inmarsat Classic Aero reads one **Channel**: P to aircraft, R/T bursts from aircraft, or C voice
circuit signalling. It plays no audio.

Iridium decodes one 50 kHz channel. A **Span** of 1, 2.5, 5 or 10 MHz decodes bursts across a
radio running at that sample rate, using the middle 80%.

## Pager text

Some German POCSAG networks send umlauts as `{ | } [ \ ] ~`. SDR-- converts them inside
lowercase words only: `M}nchen` becomes `München`, `Stra~e` becomes `Straße`. `[ALARM]` and
all-caps messages stay as sent.

## SSTV

Tune SSTV to the USB dial frequency. A picture takes from 36 seconds to four and a half minutes.

| Setting | Does |
|---|---|
| Mode | **Follow VIS** reads the mode from the transmission. Pick one when the header was missed. |
| Slant | Straightens pictures from a sender whose clock runs off. Leave it on. |
| Partial | Keeps a picture cut short by a fade |

Modes: Robot 36 and 72, Martin M1 and M2, Scottie S1, S2 and DX, PD50, PD90, PD120, PD180,
Wraase SC2-180.

Wire `video` to **Video** to watch a picture arrive. Finished pictures are kept as PNG on the
server, even with no client open, for 24 hours, up to 512 pictures or 256 MB.

## Weather satellites

**NOAA APT** decodes the analog picture on 137 MHz: both AVHRR channels side by side, with the
channel numbers read from the telemetry wedges. **Partial** keeps a pass cut short.

**Meteor LRPT** decodes the digital picture from Meteor-M on 137.9 MHz. It composes channels 64
and 65 into colour, or shows the strongest single channel. Set **Mode** to QPSK 72k, OQPSK 72k or
OQPSK 80k to match the satellite.

A picture starts when the signal locks and is saved when the pass ends, in the same store as SSTV.
Use a [Satellite](satellites.md) node to follow Doppler and know when a pass begins.

## WEFAX

Tune WEFAX to the USB carrier, 1.9 kHz below the published frequency. The start tone picks the
IOC, phasing lines set the line start and straighten the slant, and the stop tone ends the chart.

| Setting | Does |
|---|---|
| IOC | 576 or 288, used when a chart starts without a start tone |
| LPM | Lines per minute: 60, 90, 120 or 240. Most stations use 120. |
| Partial | Keeps a chart cut short by a fade |

## Radiosondes

The Radiosonde channel reads RS41, DFM, M10, M20 and iMet-4 weather balloons. **Sonde** on
**Auto** runs every type at once. Each frame gives serial, position, altitude, climb and, where the
sonde sends it, temperature, humidity and pressure. Wire it to **Map** for the flight track and to
**Decoder log** or **Readout** for the readings. RS41 temperature and humidity appear once its
calibration data has arrived, about a minute after first lock.

## APRS weather

The APRS channel reads weather reports, positioned or positionless, into wind, gust, temperature,
rain, snow, humidity, pressure and luminosity in metric units. The APRS readout lists each weather
station with its latest values and the extremes seen.

## DAB and DAB+

Wire `audio` to a Speaker. **Type** limits the choice to DAB or DAB+. **Mode** picks transmission
mode I to IV, or **Auto** detects it; it starts on I. **Service** picks a service; Auto plays the
first playable one. All run at 2.048 MS/s. Tuner offsets up to 40 kHz are corrected and shown as
frequency error.

A **Readout** shows the dynamic label and slideshow. The **Decoder log** keeps received MOT
objects with a download link. Files are offered for download, never opened in the interface.

Packet services appear in the same list as audio services. IP services emit datagrams, see
[IP data](network-iq.md#ip-data).

## DRM

Wire `audio` to a Speaker. **Mode** picks DRM30 (robustness A to D), DRM+ (E) or Auto. Auto
covers the DRM+ width. **BW** sets the DRM30 channel, 4.5 to 20 kHz; tune to the DRM reference
frequency. **Service** picks one of up to four services; Auto plays the first playable one.

AAC, HE-AAC and HE-AAC v2 play. xHE-AAC needs an FFmpeg with USAC. The dynamic label shows the
text message. SDC and text CRC failures count as data failures, broken audio frames as audio
failures.

## DVB

DVB-T/T2 and DVB-S/S2 play the chosen programme's first audio and video streams. Wire `audio` to
a Speaker and `video` to a **Video** node. **Service** picks a discovered programme or takes its
number; Auto plays the first.

**DVB-T/T2:** set **Standard** and **BW**. Everything else is read from the signal.
**Low priority** picks the DVB-T LP stream. **PLP** picks a DVB-T2 pipe, or the first TS pipe if
left empty. **1.7 MHz** fits a 2.048 MS/s radio.

**DVB-S/S2:** set **Rate** from 100 kBd to 4 MBd. The radio must be wider than the carrier,
symbol rate × (1 + roll-off): 2.7 MHz for 2 MBd at 0.35. On DVB-S, **FEC** sets the code rate or
finds it on Auto, and roll-off is always 0.35. On DVB-S2, set **Roll-off** to match the
transmitter; the MODCOD is found automatically, including VL-SNR. **Stream** picks one input
stream on a multistream carrier. GSE packets can go to your network, see
[IP data](network-iq.md#ip-data).

**Superframes** enables DVB-S2X Annex E, formats 0 to 7. Walsh-Hadamard rows are found
automatically. Set **Ref code** and **Data code** when the carrier does not use the default
scrambling, or turn on **Code search** to find them on a clean signal.

## DECT

The DECT channel surveys base stations: identity, capabilities, and security. It reads signalling
only, never call audio.

It needs a radio that reaches 1.9 GHz and is wider than one 1.728 MHz carrier. HackRF and SDRplay
work, RTL-SDR does not.

| Setting | Choice |
|---|---|
| Band | EU 1880 to 1900 MHz, or US 1920 to 1930 MHz |
| Side | Base, Handset, or Both |

Carriers are 1.728 MHz apart. EU carrier 0 is 1897.344 MHz and the numbers count down. US
carriers count up from 1921.536 MHz.

Each record lists the base identity (RFPI), system information, capabilities, advertised and
observed security, and handset IDs seen during encryption setup. Encryption is marked active only
after a grant is seen. Advertised support does not prove a call was encrypted, and missing
signalling does not prove it was not.
