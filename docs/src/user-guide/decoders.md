# Decoders

Every mode is a [channel](channels.md). This page lists them, says how well each is tested, and
covers the modes that need more than a frequency.

## Catalog

**+ Node** lists the modes in the running build.

| Group | Tested on air | Fixture only | Experimental |
|---|---|---|---|
| Analog voice | AM, NFM, SSB, WFM with stereo and RDS | | |
| Digital voice | DMR, FreeDV 1600 | D-STAR, System Fusion, NXDN, P25 Phase 1, dPMR, M17 | |
| Aviation | ADS-B (1090ES) | ACARS, VDL Mode 2, HFDL, Inmarsat Classic Aero | VOR, ILS localizer and glideslope |
| Marine | | AIS, NAVTEX, DSC, Inmarsat STD-C and EGC | |
| Amateur and HF | CW skimmer, FT8, FT4, WSPR | APRS / AX.25, RTTY, PSK31 to PSK250, Morse | |
| Paging and telemetry | POCSAG | FLEX, ERMES, Selcall (CCIR, ZVEI), Sub-GHz OOK/FSK, [ISM sensors](#ism-sensors), DCF77, WWVB, MSF, JJY | |
| Pictures and video | | [SSTV](#sstv), ATV | |
| Broadcast digital | [DAB and DAB+](#dab-and-dab) | | [DVB-T/T2, DATV (DVB-S/S2)](#dvb), DRM30 and DRM+ |
| Utility | [Signal identifier](scanning.md#identify-a-signal) | Iridium bursts, [DECT survey](#dect) | GNSS lab (GPS L1 C/A) |

| Label | Means |
|---|---|
| Tested on air | Verified live, through a real radio and the full receiver |
| Fixture only | Verified on recordings, generated IQ, or reference vectors. Not yet verified live. |
| Experimental | Partly works. See the limits below. |

Fixtures catch decoding bugs but say little about drift, fading, or interference. The
[fixture library](https://github.com/Newspicel/sdrminusminus/blob/main/fixtures/README.md) lists
where each recording came from. VDL Mode 2, HFDL, Inmarsat Classic Aero and STD-C, and DSC use
[xng](https://github.com/airframesio/xng).

Have a short on-air recording of a fixture-only mode, with the decoded output? It is the most
useful contribution there is. See [Build and test](../development/building.md).

### Experimental limits

| Mode | Works | Missing |
|---|---|---|
| DATV | DVB-S/S2/S2X, programme tables, audio, video, GSE | Verified on synthetic IQ only |
| DVB-T/T2 | DVB-T HP/LP, T2-Base and Lite, SISO/MISO, 1K to 32K, PLP choice, audio, video | Synthetic IQ only. No GSE, no multi-RF TFS. |
| DRM30 / DRM+ | Lock, SNR, frequency error | No FAC, SDC, or MSC. No services or audio. |
| GNSS lab | GPS L1 C/A acquisition and navigation data | No position fix |
| VOR / ILS | Radial, difference in depth of modulation | Tested on generated signals only |

## DAB and DAB+

Wire `audio` to a Speaker. **Auto** plays the first audio service. **Generation** limits the
choice to DAB or DAB+. **Transmission** picks mode I to IV. All run at 2.048 MS/s. Only mode I
has been received on air.

A **Readout** shows the dynamic label and slideshow. The **Decoder log** keeps received MOT
objects with a download link. Files are offered for download, never opened in the interface.

Packet services appear in the same list as audio services. IP services emit datagrams, see
[IP data](#ip-data).

## DVB

DVB-T/T2 and DVB-S/S2 play the chosen programme's first audio and video streams. Wire `audio` to
a Speaker and `video` to a **Video** node. Pick a discovered programme or enter its number.

**DVB-T/T2:** set **Standard** and **Bandwidth**. Everything else is read from the signal.
**Low priority stream** picks DVB-T LP. **PLP** picks a DVB-T2 pipe, or the first TS pipe if left
empty. **1.7 MHz** fits a 2.048 MS/s radio.

**DVB-S/S2:** set **Symbol rate** from 100 kBd to 4 MBd. The channel rate is twice the symbol
rate, so a 2 MBd carrier needs a radio that delivers 4 MS/s. Set **Roll-off** to match the
transmitter. DVB-S is always 0.35. DVB-S2 finds the MODCOD on its own, including VL-SNR.
**Input stream** picks one stream on a multistream carrier.

**Superframes** enables DVB-S2X Annex E formats 0 and 1. Formats 2 to 7 are not supported.

## IP data

DAB IP services and DVB-S2 GSE carry IP packets. To put them on your network, wire the channel's
`events` to an **Event output**, choose **Network interface**, and set an interface name, address,
and prefix. SDR-- creates a TUN interface and writes the packets to it.

| System | Needs |
|---|---|
| Linux | `CAP_NET_ADMIN` for the server |
| macOS | Permission to create a `utun` interface; name it like `utun8` |
| Windows | Administrator rights and [wintun.dll](https://www.wintun.net/) beside the executable |

Routing and multicast are up to your operating system.

## DMR trunking

Add **DMR trunk system**, wire Device `iq`, and enter the control channel in MHz. Pick the system
type or leave it on auto. The node creates the DMR channels it needs.

| System | Finds its channels by |
|---|---|
| Tier III, including Capacity Max | Reading channel definitions from the control channel |
| Capacity Plus | Watching for carriers that share rest-channel changes |
| Hytera XPT | Same as Capacity Plus, with XPT signalling |

Following runs on the server with no browser open. Voice channels must fit inside the Device's
window. A grant outside it is reported.

**Record calls** keeps finished calls and their audio in memory. Encrypted calls keep only
metadata.

## SSTV

Tune SSTV to the SSB carrier. A picture takes from 36 seconds to four and a half minutes.

| Setting | Does |
|---|---|
| Follow VIS | Reads the mode from the transmission |
| Manual mode | Uses the chosen mode when the header was missed |
| Slant correction | Straightens pictures from a slightly off clock. Leave it on. |
| Keep unfinished pictures | Saves a picture cut short by a fade |

Modes: Robot 36 and 72, Martin M1 and M2, Scottie S1, S2 and DX, PD50 to PD180, Wraase SC2-180.

Wire `video` to **Video** to watch a picture arrive. Finished pictures are saved as PNG on the
server, even with no client open, and kept for 24 hours, up to 512 pictures.

## DECT

The DECT channel surveys base stations: identity, capabilities, and security. It reads signalling
only, never call audio.

It needs a radio that reaches 1.9 GHz at 2.304 MS/s or more. HackRF and SDRplay work, RTL-SDR does
not.

| Setting | Choice |
|---|---|
| Band | Europe 1880 to 1900 MHz, or US 1920 to 1930 MHz |
| Side | Base, Handset, or Both |

Carriers are 1.728 MHz apart. European carrier 0 is 1897.344 MHz and the numbers count down.
US carriers count up from 1921.536 MHz.

Each record lists the base identity (RFPI), system information, capabilities, advertised and
observed security, and handset IDs seen during encryption setup. Encryption is marked active only
after a grant is seen. Advertised support does not prove a call was encrypted, and missing
signalling does not prove it was not.

## ISM sensors

The Sub-GHz channel decodes known sensors and shows raw frames for everything else.

| Coding | Sensors |
|---|---|
| Pulse position | Nexus-T/TH, Rubicson (also Solight TE44, EMOS E0107T), Acurite 609TXC and 606TX, Prologue-TH, inFactory-TH, Kedsum-TH, Springfield soil probe |
| Pulse width | LaCrosse TX141TH-Bv2, Fine Offset WH2, Auriol HG02832, Geevon TX16-3, WS2032, EMOS E6016, Rubicson 48942, WT0124, Opus XT300 |
| Manchester | Ambient Weather F007TH |
| FSK | Ambient Weather WH31E, Renault TPMS, Toyota TPMS |
| Differential Manchester | WT450-TH |

A reading is shown only after its checksum passes. Decoding follows
[rtl_433](https://github.com/merbanan/rtl_433) (GPL-2.0-or-later).

## Pager text

Some German POCSAG networks send umlauts as `{ | } [ \ ] ~`. SDR-- converts them inside
lowercase words only: `M}nchen` becomes `München`, `Stra~e` becomes `Straße`. `[ALARM]` and
all-caps messages stay as sent.
