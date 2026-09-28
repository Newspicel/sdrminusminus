import Foundation
import SdrmmCore

nonisolated enum FakeScenarios {
    static let code = "48210937"
    static let fingerprintShort = "1A2B 3C4D 5E6F 7081 92A3"
    static let fingerprint = String(repeating: "1a2b3c4d", count: 8)
    static let origin = LatLon(lat: 52.5200, lon: 13.4050)
    static let field = WorkspaceRef(id: "field", name: "Field")
    static let lab = WorkspaceRef(id: "lab", name: "Lab")
    static let huntID = "hunt-1"
    static let dfID = "df-1"
    static let radarID = "radar-1"
    static let surveyID = "survey-1"

    static func server() -> SavedServer {
        server(name: "Lab Pi")
    }

    static func server(name: String) -> SavedServer {
        SavedServer(
            id: "s1",
            name: name,
            hosts: ["10.0.0.2:8443"],
            fingerprintShort: fingerprintShort,
            phoneId: "p1",
            pairedAt: "2026-09-28T12:00:00Z"
        )
    }

    static func offer(hosts: [String], code: String, name: String?) -> PairOffer {
        PairOffer(
            hosts: hosts,
            code: code,
            fingerprint: fingerprint,
            fingerprintShort: fingerprintShort,
            protocol: 1,
            serverName: name
        )
    }

    static func missions(workspace: WorkspaceRef = field) -> MissionsView {
        MissionsView(
            workspace: workspace,
            workspaces: [field, lab],
            missions: [
                mission(huntID, .hunt, "Fox 2m", "145.500 MHz", [.tune, .huntRun]),
                mission(
                    dfID,
                    .dfDrive,
                    "Kraken DF",
                    "3 stations",
                    [.tune, .calibrate, .clearFusion, .targetMode]
                ),
                mission(radarID, .radarWatch, "FM radar", "98.800 MHz", []),
                mission(surveyID, .survey, "Walk survey", "433.920 MHz", [.surveyRun, .surveyClear]),
                Mission(
                    id: "df-2",
                    kind: .dfDrive,
                    title: "Spare DF",
                    detail: "0 stations",
                    ready: false,
                    blocker: "No array",
                    controls: []
                ),
            ]
        )
    }

    private static func mission(
        _ id: String,
        _ kind: MissionKind,
        _ title: String,
        _ detail: String,
        _ controls: [MissionControl]
    ) -> Mission {
        Mission(
            id: id,
            kind: kind,
            title: title,
            detail: detail,
            ready: true,
            blocker: nil,
            controls: controls
        )
    }

    static func hunt(strength: Float, trend: Trend, running: Bool) -> HuntView {
        let floor: Float = -95
        let level = floor + 50 * strength
        return HuntView(
            mission: huntID,
            freqHz: 145_500_000,
            levelDb: level,
            smoothDb: level,
            floorDb: floor,
            bestDb: floor + 50,
            strength: strength,
            trend: trend,
            running: running,
            refusal: nil,
            readings: UInt64(strength * 100),
            sweep: nil
        )
    }

    static func pose(heading: Double?) -> PoseView {
        PoseView(
            headingDeg: heading,
            accuracyDeg: heading == nil ? nil : 4,
            source: heading == nil ? .none : .fused,
            align: .idle,
            sending: true,
            fixAgeMs: 400
        )
    }

    static func df(bearing: Float, heading: Double?) -> DfView {
        let relative = heading.map {
            Float((Double(bearing) - $0 + 360).truncatingRemainder(dividingBy: 360))
        }
        let target = NavPoint(at: offset(origin, bearingDeg: 215, meters: 1_200), kind: .probe)
        let estimateAt = offset(origin, bearingDeg: 137, meters: 2_500)
        return DfView(
            mission: dfID,
            state: .live,
            bearingTrueDeg: bearing,
            bearingRelDeg: relative,
            confidence: 0.62,
            sigmaDeg: 6,
            freqHz: 433_920_000,
            targetMode: .auto,
            guidance: GuidanceView(
                kind: .probe,
                headingTrueDeg: 215,
                headingRelDeg: heading.map { (215 - $0 + 360).truncatingRemainder(dividingBy: 360) },
                distanceM: 1_200
            ),
            target: target,
            estimate: EstimateView(
                at: estimateAt,
                majorM: 400,
                minorM: 150,
                axisDeg: 30,
                converged: false,
                samples: 12
            ),
            overlay: overlay(estimate: estimateAt)
        )
    }

    private static func overlay(estimate: LatLon) -> DfOverlay {
        let stations = [
            Station(id: "A", at: origin, bearings: 12),
            Station(id: "B", at: offset(origin, bearingDeg: 90, meters: 1_500), bearings: 8),
            Station(id: "C", at: offset(origin, bearingDeg: 180, meters: 1_800), bearings: 5),
        ]
        let rays = stations.map { station in
            Ray(
                from: station.at,
                to: offset(station.at, bearingDeg: bearing(station.at, estimate), meters: 5_000),
                weight: 0.8
            )
        }
        let heat = [(Float(0.95), 900.0), (0.8, 600), (0.5, 300)].map { level, radius in
            HeatBand(level: level, rings: [ring(estimate, radius: radius, points: 36)])
        }
        return DfOverlay(
            rays: rays,
            stations: stations,
            ellipse: ring(estimate, radius: 400, points: 48),
            heat: heat
        )
    }

    static func retarget() -> RetargetNotice {
        RetargetNotice(
            mission: dfID,
            target: NavPoint(at: offset(origin, bearingDeg: 200, meters: 1_500), kind: .estimate),
            movedM: 400,
            reason: .moved
        )
    }

    static func radar() -> RadarView {
        let tracks = (1...6).map(track)
        return RadarView(mission: radarID, echoes: 6, tracks: tracks, stale: false, problems: [])
    }

    private static func track(_ index: Int) -> RadarTrack {
        let scale = Float(index)
        let closing = index.isMultiple(of: 2)
        let doppler: Float = closing ? -35 : 48
        let bearing: Float? = index == 1 ? 137 : nil
        return RadarTrack(
            id: UInt32(index),
            rangeKm: scale * 4.2,
            dopplerHz: doppler * scale / 3,
            speedMps: scale * 20,
            snrDb: 20 - scale,
            closing: closing,
            coasting: false,
            bearingDeg: bearing
        )
    }

    static func radarImage() -> RgbaImage {
        let width = 64
        let height = 32
        var rgba = Data(capacity: width * height * 4)
        for row in 0..<height {
            for column in 0..<width {
                let value = UInt8((row * 255 / height + column * 255 / width) / 2)
                rgba.append(contentsOf: [value, 64, 255 - value, 255])
            }
        }
        return RgbaImage(
            width: UInt32(width),
            height: UInt32(height),
            rgba: rgba,
            rangeMaxKm: 60,
            dopplerSpanHz: 400
        )
    }

    static func survey() -> SurveyView {
        SurveyView(
            mission: surveyID,
            freqHz: 433_920_000,
            levelDb: -62,
            minDb: -90,
            maxDb: -40,
            total: 200,
            recording: true
        )
    }

    static func surveyPoints() -> [SurveyPoint] {
        (0..<200).map { index in
            let level = -90 + 50 * Float(index) / 199
            return SurveyPoint(at: offset(origin, bearingDeg: 45, meters: Double(index) * 5), levelDb: level)
        }
    }

    static func plan() -> RoutePlan {
        let corner = offset(origin, bearingDeg: 0, meters: 600)
        let end = offset(corner, bearingDeg: 90, meters: 800)
        return RoutePlanBuilder.plan(
            name: "Route 1",
            travelTimeS: 240,
            steps: [
                RawStep(instruction: "", notice: nil, distanceM: 0, points: [origin]),
                RawStep(instruction: "Go north", notice: nil, distanceM: 600, points: [origin, corner]),
                RawStep(instruction: "Turn right", notice: nil, distanceM: 800, points: [corner, end]),
                RawStep(instruction: "Arrive", notice: nil, distanceM: 0, points: [end]),
            ]
        )
    }

    static func offset(_ from: LatLon, bearingDeg: Double, meters: Double) -> LatLon {
        let radius = 6_371_000.0
        let angle = bearingDeg * .pi / 180
        let north = meters * cos(angle)
        let east = meters * sin(angle)
        let lat = from.lat + north / radius * 180 / .pi
        let lon = from.lon + east / (radius * cos(from.lat * .pi / 180)) * 180 / .pi
        return LatLon(lat: lat, lon: lon)
    }

    private static func bearing(_ from: LatLon, _ to: LatLon) -> Double {
        geoBearingDeg(from: from, to: to)
    }

    private static func ring(_ center: LatLon, radius: Double, points: Int) -> [LatLon] {
        let open = (0..<points).map { index in
            offset(center, bearingDeg: Double(index) * 360 / Double(points), meters: radius)
        }
        return open + open.prefix(1)
    }
}
