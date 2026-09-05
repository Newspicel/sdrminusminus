# Network IQ export

Network IQ sends a live stream to another analysis program without first recording it.

1. Add a **Network IQ** node.
2. Connect one Device `IQ` output or one channel `baseband` output. For multi-stream radios,
   choose the lane to export. A node cannot accept both source types at once.
3. Choose UDP or TCP, the sample encoding, and a `host:port` destination.
4. Start the receiving tool first, then press **Start export**.

The face reports the exact sample rate and center frequency to enter in the receiver, plus sent
bytes, datagram/write count, capture overruns, and writer errors. The sample rate is locked while the
export is active because the raw stream has no in-band rate-change message. Retuning remains
available; update the receiver's center-frequency setting after a retune.

Channel baseband uses the same format at the channel's lower sample rate, after frequency
translation and filtering. This reduces network bandwidth when another tool only needs one signal.
Each channel supports one export, independently of device-wide export.

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
header, so the stream cannot reliably identify missing or reordered datagrams. sdr-- reports
loss before the socket, but cannot report network delivery loss. GNU Radio's **UDP Source** should use
header `None`, the matching data type, and a payload size of 1,400.

TCP connects outward to the destination and writes one continuous byte stream. The receiving
program must be listening before export starts. TCP avoids datagram loss and reordering, but a
receiver that cannot keep up eventually fills the bounded export queue; sdr-- then stops the
writer and reports the error.

## Security

The API caller chooses the destination host and port, and an active export can send several
megabits per second. Keep the server restricted to trusted callers. In particular, configure the
[shared token](../server/configuration.md#shared-token-authentication) and appropriate network
access controls whenever the HTTP server is reachable beyond the local desktop.

## Protocol compatibility

This output is raw IQ. Configure the receiving program with the same encoding, sample rate, and
centre frequency; the stream carries no timestamps, stream IDs, or radio metadata.

It is not a [VITA 49](https://www.vita.com/page-1855484) or
[DIFI](https://dificonsortium.org/standards/) stream. Receivers expecting either protocol need
framing and context that this output does not provide.

It is also separate from `rtl_tcp`, which lets a client control a remote radio's tuning, sample
rate, and gain. sdr-- supports `rtl_tcp` as a network Device source.
