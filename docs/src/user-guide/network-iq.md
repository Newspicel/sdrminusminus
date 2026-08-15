# Network IQ export

Network IQ sends a live device stream to another analysis program without first recording it.

1. Add a **Network IQ** node.
2. Wire a Device `IQ` output into it. On a multi-stream radio, the chosen output selects the
   exported stream.
3. Choose UDP or TCP, the sample encoding, and a `host:port` destination.
4. Start the receiving tool first, then press **Start export**.

The face reports the exact sample rate and center frequency to enter in the receiver, plus sent
bytes, datagram/write count, capture overruns, and writer errors. The sample rate is locked while the
export is active because the raw stream has no in-band rate-change message. Retuning remains
available; update the receiver's center-frequency setting after a retune.

## Wire contract

The payload is unframed, interleaved `I, Q, I, Q, ...` in one of these encodings:

| Setting | Components | Bytes per complex sample | Typical receiver type |
|---|---|---:|---|
| `cf32_le` | IEEE-754 32-bit float, little-endian | 8 | GNU Radio Complex |
| `ci16_le` | signed 16-bit integer, little-endian | 4 | GNU Radio Short, then Interleaved Short to Complex |
| `cu8` | unsigned 8-bit integer, zero at 127.5 | 2 | RTL-SDR-style byte IQ |

The identifiers and byte layout follow the
[SigMF datatype definitions](https://sigmf.org/#sigmf-dataset-format), which specify interleaved
I-first complex samples. This is not itself a SigMF recording because a live stream has no SigMF
metadata file; the node reports the rate and center frequency separately.

UDP uses payloads of at most 1,400 bytes, below a normal 1,500-byte Ethernet MTU after IP and
UDP headers. Each datagram contains a whole number of complex samples. There is no sequence
header, so a receiver should treat UDP as best-effort: sdr-- can report loss before the socket,
but only the receiver can detect loss in the network. GNU Radio's **UDP Source** should use
header `None`, the matching data type, and a payload size of 1,400.

TCP connects outward to the destination and writes one continuous byte stream. The receiving
program must be listening before export starts. TCP avoids datagram loss and reordering, but a
receiver that cannot keep up eventually fills the bounded export queue; sdr-- then stops the
writer and surfaces the error rather than leaving an unmarked hole.

## Security

The API caller chooses the destination host and port, and an active export can send several
megabits per second. Keep the server restricted to trusted callers. In particular, configure the
[shared token](../server/configuration.md#shared-token-authentication) and appropriate network
access controls whenever the HTTP server is reachable beyond the local desktop.

## Standards and conventions

There is no universal “raw IQ over UDP” standard. GNU Radio's network blocks deliberately let
the two endpoints agree on item type, payload size, and an optional header. The contract above
is the widely supported raw convention and is intentionally named as such.

[VITA Radio Transport (VITA 49)](https://www.vita.com/page-1855484) is the formal packet family
for samples plus stream identity, timestamps, and radio context. The current
[DIFI standard](https://dificonsortium.org/standards/) is an interoperability profile of VITA
49.2. sdr-- does not label this output VITA-49 or DIFI: a compliant stream needs context such as
timing and calibrated reference-level information that many attached receivers do not expose.
Adding a partial header would look standardized while still requiring proprietary receiver
assumptions.

`rtl_tcp` is also not this TCP mode. It is a remote-radio protocol in which the client controls
the source's tuning, rate, and gain; a push-only analysis sink cannot implement those semantics.
sdr-- supports `rtl_tcp` separately when opening a network radio.
