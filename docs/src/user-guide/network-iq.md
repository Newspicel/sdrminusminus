# Network export

Send live IQ or decoded events to other programs.

| Node | Mode | For |
|---|---|---|
| Network IQ | UDP or TCP | GNU Radio and other raw IQ tools |
| Network IQ | rtl_tcp server | rtl_433 and other rtl_tcp clients |
| Event output | ADS-B Beast TCP | Flight tracking feeders |
| Event output | Webhook, Matrix, MQTT | Chat, automation and alerts |
| Event output | PostgreSQL, InfluxDB | Databases |
| Event output | Network interface | IP packets from DAB and DVB-S2 |

Network IQ and Beast ports skip the server's token and TLS. Keep them on trusted networks. A
listener address belongs to the server: use its LAN address or `0.0.0.0` to accept other
machines.

## Raw IQ over UDP or TCP

1. Add **Network IQ** and wire one Device `iq` lane or one channel `baseband`.
2. Pick **Transport**, **Samples**, and the destination `host:port` in **To**.
3. Start the receiving program, then press **Start export**.
4. Set the receiver to the rate and centre frequency shown on the node.

Channel `baseband` sends only that channel, at its lower rate. The sample rate is locked during
export. Retuning works, but you must update the receiver yourself. The node counts bytes sent,
datagrams or writes, and drops.

Samples are interleaved `I, Q, I, Q`, with no header or timestamps:

| Samples | Encoding | Bytes per I/Q pair | GNU Radio type |
|---|---|---:|---|
| Complex float 32 LE | `cf32_le`, 32-bit float | 8 | Complex |
| Complex int 16 LE | `ci16_le`, signed 16-bit | 4 | Short, then Interleaved Short to Complex |
| Complex unsigned 8 | `cu8`, unsigned 8-bit, zero at 127.5 | 2 | RTL-SDR byte IQ |

Names follow [SigMF](https://sigmf.org/#sigmf-dataset-format). VITA 49 and DIFI are not supported.

**UDP datagrams** sends whole samples in datagrams of up to 1,400 bytes. In GNU Radio's
**UDP Source**, set header to `None` and payload size to 1,400. There are no sequence numbers, so
lost packets cannot be detected.

**TCP stream** connects to your listening program. If it reads too slowly, the export stops with
an error.

## rtl_433

1. Tune the Device to **433.92 MHz**.
2. Wire Device `iq` into **Network IQ** and pick **rtl_tcp server (rtl_433)**. It listens on
   `127.0.0.1:1234`.
3. Press **Start export** and run rtl_433 with the rate and frequency shown:

```sh
rtl_433 -d rtl_tcp:127.0.0.1:1234 -s 1024000 -f 433920000 -F json
```

A channel's `baseband` works too if it covers the sensor. Commands from the client cannot retune
the radio; set that on the canvas. Up to eight clients can connect. A slow one is dropped without
affecting the others.

## ADS-B Beast

1. Wire the ADS-B channel's `events` into **Event output**.
2. Pick **ADS-B Beast TCP**, set **Listen** to `127.0.0.1:30005`, and press **Open server**.
3. Point your feeder at that address.

The server sends [Beast binary frames](https://wiki.jetvision.de/wiki/Mode-S_Beast:Data_Output_Formats)
with 12 MHz timestamps and signal levels. Timestamps count samples, not GPS time, and restart
after gaps. Use one ADS-B channel per output. Up to 16 clients can connect.

## Webhooks, Matrix and MQTT

Wire `events` to an **Event output** and pick a **Service**. Each event is sent as it arrives.

| Service | Sends |
|---|---|
| Webhook, JSON | An HTTP POST with `output`, `kind`, `text`, and the full `record` |
| Webhook, Discord | The text as a message, with any audio attached |
| Matrix | The text to a room, with any audio uploaded |
| MQTT | The JSON payload to a topic, at least once. Use `mqtt://` or `mqtts://`. |

Long messages are cut at 1,900 characters. A rate-limited send is retried up to four times.
Failures go to the server log.

## Databases

Wire `events` to an **Event output** and choose **PostgreSQL** or **InfluxDB**.

**PostgreSQL** creates the table (default `sdrmm_events`) on first write: one row per event with time, kind, frequency,
station, summary, and the full record as `jsonb`. Add `?sslmode=disable` to the URL for a server
without TLS.

**InfluxDB** 2 and 3 take one point per event. The measurement is the event kind, and numbers,
flags, and short text from the event become fields.

## IP data

DAB IP services and DVB-S2 GSE carry IP packets. Wire the channel's `events` to an
**Event output**, choose **Network interface**, and set **Interface**, **IPv4**, and **Prefix**.
SDR-- creates a TUN interface and writes the packets to it.

| System | Needs |
|---|---|
| Linux | `CAP_NET_ADMIN` for the server |
| macOS | Permission to create a `utun` interface; name it like `utun8` |
| Windows | Administrator rights and [wintun.dll](https://www.wintun.net/) beside the executable |

Routing and multicast are up to your operating system.
