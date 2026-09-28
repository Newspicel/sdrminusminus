package dev.newspicel.sdrmm.demo

import dev.newspicel.sdrmm.ffi.AlignState
import dev.newspicel.sdrmm.ffi.DfOverlay
import dev.newspicel.sdrmm.ffi.DfState
import dev.newspicel.sdrmm.ffi.DfView
import dev.newspicel.sdrmm.ffi.EstimateView
import dev.newspicel.sdrmm.ffi.GuidanceKind
import dev.newspicel.sdrmm.ffi.GuidanceView
import dev.newspicel.sdrmm.ffi.HeadingSourceKind
import dev.newspicel.sdrmm.ffi.HuntView
import dev.newspicel.sdrmm.ffi.LatLon
import dev.newspicel.sdrmm.ffi.Mission
import dev.newspicel.sdrmm.ffi.MissionControl
import dev.newspicel.sdrmm.ffi.MissionKind
import dev.newspicel.sdrmm.ffi.MissionsView
import dev.newspicel.sdrmm.ffi.NavPoint
import dev.newspicel.sdrmm.ffi.PoseView
import dev.newspicel.sdrmm.ffi.RadarTrack
import dev.newspicel.sdrmm.ffi.RadarView
import dev.newspicel.sdrmm.ffi.Ray
import dev.newspicel.sdrmm.ffi.RgbaImage
import dev.newspicel.sdrmm.ffi.SavedServer
import dev.newspicel.sdrmm.ffi.TargetMode
import dev.newspicel.sdrmm.ffi.Trend
import dev.newspicel.sdrmm.ffi.WorkspaceRef
import dev.newspicel.sdrmm.sensors.Declination
import kotlin.math.atan2
import kotlin.math.cos
import kotlin.math.exp
import kotlin.math.sin

class DemoScript {
    @Volatile private var huntHz = HUNT_HZ

    fun server(): SavedServer = SavedServer(
        id = DemoGateway.SERVER_ID,
        name = DemoGateway.SERVER_NAME,
        hosts = emptyList(),
        fingerprintShort = "",
        phoneId = "",
        pairedAt = "",
    )

    fun missions(): MissionsView {
        val workspace = WorkspaceRef(id = "demo", name = "Demo")
        return MissionsView(
            workspace = workspace,
            workspaces = listOf(workspace),
            missions =
            listOf(
                Mission(
                    HUNT_ID,
                    MissionKind.HUNT,
                    "Beacon",
                    "433.920 MHz",
                    true,
                    null,
                    listOf(MissionControl.TUNE, MissionControl.HUNT_RUN),
                ),
                Mission(
                    DF_ID,
                    MissionKind.DF_DRIVE,
                    "Kraken DF",
                    "145.500 MHz",
                    true,
                    null,
                    listOf(MissionControl.TUNE, MissionControl.CALIBRATE, MissionControl.CLEAR_FUSION, MissionControl.TARGET_MODE),
                ),
                Mission(RADAR_ID, MissionKind.RADAR_WATCH, "FM radar", "98.800 MHz", true, null, emptyList()),
            ),
        )
    }

    fun kindOf(id: String?): MissionKind? = missions().missions.firstOrNull { it.id == id }?.kind

    fun pose(): PoseView = PoseView(
        headingDeg = 42.0,
        accuracyDeg = 8.0,
        source = HeadingSourceKind.FUSED,
        align = AlignState.Idle,
        sending = false,
        fixAgeMs = null,
    )

    fun tune(hz: Double) {
        huntHz = hz
    }

    fun hunt(
        tick: Long,
        running: Boolean,
    ): HuntView {
        val strength = (0.5 + 0.45 * sin(tick / 8.0)).toFloat()
        val level = (FLOOR_DB + strength * SPAN_DB)
        val rising = cos(tick / 8.0) > 0
        return HuntView(
            mission = HUNT_ID,
            freqHz = huntHz,
            levelDb = level,
            smoothDb = level,
            floorDb = FLOOR_DB,
            bestDb = FLOOR_DB + SPAN_DB,
            strength = strength,
            trend =
            when {
                !running -> Trend.WAITING
                strength > ON_TOP -> Trend.ON_TOP
                rising -> Trend.WARMER
                else -> Trend.COLDER
            },
            running = running,
            refusal = null,
            readings = tick.toULong(),
            sweep = null,
        )
    }

    fun df(tick: Long): DfView {
        val bearing = ((tick * 3) % 360).toFloat()
        val target = LatLon(ORIGIN.lat + 0.01, ORIGIN.lon + 0.015)
        return DfView(
            mission = DF_ID,
            state = DfState.LIVE,
            bearingTrueDeg = bearing,
            bearingRelDeg = ((bearing - 42f + 360f) % 360f),
            confidence = 0.7f,
            sigmaDeg = 4f,
            freqHz = DF_HZ,
            targetMode = TargetMode.AUTO,
            guidance = GuidanceView(GuidanceKind.ESTIMATE, bearingDeg(ORIGIN, target), null, distanceM(ORIGIN, target)),
            target = NavPoint(target, GuidanceKind.ESTIMATE),
            estimate = EstimateView(target, 250.0, 120.0, 30.0, tick > CONVERGED_AFTER, (tick / 2).toUInt()),
            overlay = DfOverlay(rays = listOf(Ray(ORIGIN, target, 1f)), stations = emptyList(), ellipse = emptyList(), heat = emptyList()),
        )
    }

    fun radar(tick: Long): RadarView = RadarView(
        mission = RADAR_ID,
        echoes = (3 + tick % 4).toUInt(),
        tracks =
        (1..3).map { id ->
            val range = (8.0 + id * 4 + sin((tick + id * 10) / 12.0) * 2).toFloat()
            val doppler = (id * 25 * cos((tick + id * 7) / 15.0)).toFloat()
            RadarTrack(id.toUInt(), range, doppler, doppler * 1.5f, 20f - id * 3, doppler < 0, false, null)
        },
        stale = false,
        problems = emptyList(),
    )

    fun image(tick: Long): RgbaImage {
        val pixels = ByteArray(IMAGE_W * IMAGE_H * 4)
        val cx = (IMAGE_W / 2 + (IMAGE_W / 3) * sin(tick / 10.0)).toInt()
        val cy = (IMAGE_H / 2 + (IMAGE_H / 3) * cos(tick / 13.0)).toInt()
        for (y in 0 until IMAGE_H) {
            for (x in 0 until IMAGE_W) {
                val d = ((x - cx) * (x - cx) + (y - cy) * (y - cy)).toDouble()
                val v = (255 * exp(-d / BLOB)).toInt().coerceIn(BACKGROUND, 255)
                val at = (y * IMAGE_W + x) * 4
                pixels[at] = (v / 3).toByte()
                pixels[at + 1] = v.toByte()
                pixels[at + 2] = (v / 2 + BACKGROUND).coerceAtMost(255).toByte()
                pixels[at + 3] = 255.toByte()
            }
        }
        return RgbaImage(IMAGE_W.toUInt(), IMAGE_H.toUInt(), pixels, 30f, 200f)
    }

    companion object {
        const val HUNT_ID = "demo-hunt"
        const val DF_ID = "demo-df"
        const val RADAR_ID = "demo-radar"
        private const val HUNT_HZ = 433.92e6
        private const val DF_HZ = 145.5e6
        private const val FLOOR_DB = -95f
        private const val SPAN_DB = 40f
        private const val ON_TOP = 0.9f
        private const val CONVERGED_AFTER = 40L
        private const val IMAGE_W = 64
        private const val IMAGE_H = 48
        private const val BLOB = 30.0
        private const val BACKGROUND = 20
        private val ORIGIN = LatLon(52.52, 13.405)

        fun distanceM(
            from: LatLon,
            to: LatLon,
        ): Double = Declination.distanceM(from.lat, from.lon, to.lat, to.lon)

        fun bearingDeg(
            from: LatLon,
            to: LatLon,
        ): Double {
            val lat1 = Math.toRadians(from.lat)
            val lat2 = Math.toRadians(to.lat)
            val dLon = Math.toRadians(to.lon - from.lon)
            val y = sin(dLon) * cos(lat2)
            val x = cos(lat1) * sin(lat2) - sin(lat1) * cos(lat2) * cos(dLon)
            return (Math.toDegrees(atan2(y, x)) + 360.0) % 360.0
        }
    }
}
