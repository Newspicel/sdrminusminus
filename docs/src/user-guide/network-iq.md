# Network export

Send live IQ or decoded aircraft to other programs.

| Node | Mode | For |
|---|---|---|
| Network IQ | UDP or TCP | GNU Radio and other raw IQ tools |
| Network IQ | rtl_tcp server | rtl_433 and other rtl_tcp clients |
| Event output | ADS-B Beast TCP | Flight tracking feeders |

None of these ports use the server's token or TLS. Keep them on trusted networks. A listener
address belongs to the server: use its LAN address or `0.0.0.0` to accept other machines.

## Raw IQ over UDP or TCP

1. Add **Network IQ** and wire one Device `iq` lane or one channel `baseband`.
2. Pick the protocol, encoding, and destination `host:port`.
3. Start the receiving program, then press **Start export**.
4. Set the receiver to the rate and centre frequency shown on the node.

Channel `baseband` sends only that channel, at its lower rate. The sample rate is locked during
export. Retuning works, but you must update the receiver yourself. The node counts bytes, writes,
and errors.

Samples are interleaved `I, Q, I, Q`, with no header or timestamps:

| Encoding | Samples | Bytes per I/Q pair | GNU Radio type |
|---|---|---:|---|
| `cf32_le` | 32-bit float | 8 | Complex |
| `ci16_le` | Signed 16-bit | 4 | Short, then Interleaved Short to Complex |
| `cu8` | Unsigned 8-bit, zero at 127.5 | 2 | RTL-SDR byte IQ |

Names follow [SigMF](https://sigmf.org/#sigmf-dataset-format). VITA 49 and DIFI are not supported.

**UDP** sends whole samples in datagrams of up to 1,400 bytes. In GNU Radio's **UDP Source**, set
header to `None` and payload size to 1,400. There are no sequence numbers, so lost packets cannot
be detected.

**TCP** connects to your listening program. If it reads too slowly, the export stops with an
error.

## rtl_433

1. Tune the Device to **433.92 MHz**.
2. Wire Device `iq` into **Network IQ** and pick **rtl_tcp server (rtl_433)** on `127.0.0.1:1234`.
3. Press **Start export** and run rtl_433 with the rate and frequency shown:

```sh
rtl_433 -d rtl_tcp:127.0.0.1:1234 -s 1024000 -f 433920000 -F json
```

A channel's `baseband` works too if it covers the sensor. Commands from the client cannot retune
the radio; set that on the canvas. Up to eight clients can connect. A slow one is dropped without
affecting the others.

## ADS-B Beast

1. Wire the ADS-B channel's `events` into **Event output**.
2. Pick **ADS-B Beast TCP**, set `127.0.0.1:30005`, and press **Open server**.
3. Point your feeder at that address.

The server sends [Beast binary frames](https://wiki.jetvision.de/wiki/Mode-S_Beast:Data_Output_Formats)
with 12 MHz timestamps and signal levels. Timestamps count samples, not GPS time, and restart
after gaps. Use one ADS-B channel per output. Up to 16 clients can connect.
