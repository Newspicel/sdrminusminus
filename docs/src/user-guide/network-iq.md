# Network IQ export

Send live IQ over UDP or TCP, or open an rtl_tcp server for rtl_433.

## Start an export

1. Add **Network IQ**.
2. Connect one Device `IQ` lane or one channel `baseband` output.
3. Choose the protocol, sample encoding, and destination as `host:port`.
4. Start the receiving program, then press **Start export**.
5. Enter the displayed sample rate and centre frequency in the receiver.

A node accepts one source. Channel baseband exports filtered IQ at the channel's lower rate;
each channel supports one export, independently of device-wide export.

Sample rate is locked during export. Retuning remains available, but you must update the receiving
program's centre frequency. The display reports sent bytes, writes, and errors.

## Wire contract

Raw UDP and TCP payloads contain unframed, interleaved `I, Q, I, Q, ...` samples:

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

There are no sequence numbers. SDR-- reports loss before the socket but cannot detect missing or
reordered network datagrams.

### TCP

SDR-- connects to a listening receiver and writes a continuous byte stream. TCP preserves order
and delivery, but a slow receiver can fill the bounded export queue. The export then stops and
reports an error.

## Access control

Exports can use substantial bandwidth and send to caller-selected destinations. Restrict server
access to trusted operators with [authentication](../server/configuration.md#shared-token-authentication)
and network controls.

## Protocol compatibility

The output is raw IQ. Receiving software must accept the selected encoding and use the displayed
rate and frequency. VITA 49 and DIFI framing are not supported.

### rtl_433

1. Tune the Device to **433.92 MHz** and choose a supported sample rate.
2. Wire Device `IQ` into **Network IQ**.
3. Select **rtl_tcp server (rtl_433)** and listen on `127.0.0.1:1234`.
4. Press **Start export**, then run rtl_433 with the displayed rate and frequency:

```sh
rtl_433 -d rtl_tcp:127.0.0.1:1234 -s 1024000 -f 433920000 -F json
```

Replace `1024000` with the export's actual sample rate. Channel `baseband` also works when its
bandwidth covers the sensor signal. The server sends an `RTL0` header followed by CU8 IQ,
matching [rtl_433's rtl_tcp input](https://github.com/merbanan/rtl_433/blob/master/src/sdr.c).

Client commands do not tune or change gain on the wired source. Configure those on the canvas;
keep rtl_433's rate and frequency aligned. Up to eight clients can connect or reconnect.
A slow client is disconnected and reported without stopping the radio or other clients.

### ADS-B Beast

1. Wire the ADS-B channel's `events` output into **Event output**.
2. Select **ADS-B Beast TCP**, set `127.0.0.1:30005`, and press **Open server**.
3. Connect your feeder or other Beast-compatible receiver to that address.

The server sends [Beast binary frames](https://wiki.jetvision.de/wiki/Mode-S_Beast:Data_Output_Formats),
including short Mode S replies, 12 MHz sample timestamps, signal levels, and byte escaping.
Only ADS-B events reaching this node are exported. Use one ADS-B source per Beast output;
sample clocks from separate channels are independent. Timestamps are relative to the decoder's
sample stream, without GPS synchronization or continuity across capture gaps and restarts.

The node reports clients, delivered frames, and errors. Up to 16 clients can connect;
slow clients are disconnected. **Close server**, removing its event wire, or deleting the node
closes the listener and clients.

Listener addresses belong to the SDR-- server machine. Use its LAN address or `0.0.0.0` to accept
remote clients. These TCP exports have no authentication or encryption; restrict them to trusted
networks. The web API token does not protect these ports.
