# Network IQ export

Send live IQ to another analysis program over UDP or TCP.

## Start an export

1. Add **Network IQ**.
2. Connect one Device `IQ` lane or one channel `baseband` output.
3. Choose the protocol, sample encoding, and destination as `host:port`.
4. Start the receiving program, then press **Start export**.
5. Enter the displayed sample rate and centre frequency in the receiver.

A node accepts one source. Channel baseband exports filtered IQ at the channel's lower rate;
each channel supports one export, independently of device-wide export.

Sample rate is locked during export. Retuning remains available, but you must update the receiving
program's centre frequency. The display reports sent bytes, writes, capture overruns, and errors.

## Wire contract

Payloads contain unframed, interleaved `I, Q, I, Q, ...` samples:

| Encoding | Components | Bytes per complex sample | GNU Radio input |
|---|---|---:|---|
| `cf32_le` | Little-endian 32-bit float | 8 | Complex |
| `ci16_le` | Little-endian signed 16-bit integer | 4 | Short, then Interleaved Short to Complex |
| `cu8` | Unsigned 8-bit integer, zero at 127.5 | 2 | RTL-SDR-style byte IQ |

Encoding names follow [SigMF datatypes](https://sigmf.org/#sigmf-dataset-format).
The stream carries no metadata, timestamps, or stream IDs.

### UDP

Datagrams contain whole complex samples, with payloads up to 1,400 bytes. Configure GNU Radio's
**UDP Source** with header `None`, matching data type, and payload size 1,400.

There are no sequence numbers. sdr-- reports loss before the socket but cannot detect missing or
reordered network datagrams.

### TCP

sdr-- connects to a listening receiver and writes a continuous byte stream. TCP preserves order
and delivery, but a slow receiver can fill the bounded export queue. The export then stops and
reports an error.

## Access control

Exports can use substantial bandwidth and send to caller-selected destinations. Restrict server
access to trusted operators with [authentication](../server/configuration.md#shared-token-authentication)
and network controls.

## Protocol compatibility

The output is raw IQ. Receiving software must accept the selected encoding and use the displayed
rate and frequency. VITA 49 and DIFI framing are not supported.

`rtl_tcp` is a separate protocol for controlling remote radios. sdr-- supports it as a Device source.
