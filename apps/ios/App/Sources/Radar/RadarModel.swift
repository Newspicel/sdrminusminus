import CoreGraphics
import Foundation
import Observation
import SdrmmCore
import os

struct RadarRow: Identifiable, Equatable {
    let id: UInt32
    let name: String
    let range: String
    let doppler: String
    let motion: String
    let bearing: String?
    let closing: Bool
    let coasting: Bool
}

struct RadarAxes: Equatable {
    let rangeMaxKm: Float
    let dopplerSpanHz: Float

    var rangeEnd: String { rangeMaxKm.isFinite ? String(format: "%.0f km", rangeMaxKm) : "-" }
    var dopplerTop: String { RadarText.doppler(dopplerSpanHz / 2) }
    var dopplerBottom: String { RadarText.doppler(-dopplerSpanHz / 2) }
}

nonisolated enum RadarText {
    static func name(_ id: UInt32) -> String { String(format: "T%02d", id) }
    static func range(_ km: Float) -> String { km.isFinite ? String(format: "%.1f km", km) : "-" }
    static func doppler(_ hz: Float) -> String { hz.isFinite ? String(format: "%+.0f Hz", hz) : "-" }
    static func echoes(_ count: UInt32) -> String { count == 1 ? "1 echo" : "\(count) echoes" }
}

@Observable
final class RadarModel {
    static let maxRows = 8

    private(set) var view: RadarView?
    private(set) var image: CGImage?
    private(set) var imageFailed = false
    private(set) var axes: RadarAxes?
    @ObservationIgnored private var logged: Set<String> = []

    func apply(_ view: RadarView) {
        self.view = view
    }

    func apply(image: RgbaImage) {
        do throws(RadarImageError) {
            self.image = try RadarImageRenderer.cgImage(image)
            imageFailed = false
            axes = RadarAxes(rangeMaxKm: image.rangeMaxKm, dopplerSpanHz: image.dopplerSpanHz)
        } catch {
            self.image = nil
            imageFailed = true
            if logged.insert(error.kind).inserted {
                Log.core.error("radar image failed: \(String(describing: error), privacy: .public)")
            }
        }
    }

    func missionOpened() {
        reset()
    }

    func missionClosed() {
        reset()
    }

    var rows: [RadarRow] {
        let tracks = view?.tracks ?? []
        return tracks.sorted { Self.rank($0) > Self.rank($1) }.prefix(Self.maxRows).map(Self.row)
    }

    private func reset() {
        view = nil
        image = nil
        imageFailed = false
        axes = nil
    }

    private static func rank(_ track: RadarTrack) -> Float {
        track.snrDb.isNaN ? -.infinity : track.snrDb
    }

    private static func row(_ track: RadarTrack) -> RadarRow {
        RadarRow(
            id: track.id,
            name: RadarText.name(track.id),
            range: RadarText.range(track.rangeKm),
            doppler: RadarText.doppler(track.dopplerHz),
            motion: track.closing ? "Closing" : "Opening",
            bearing: track.bearingDeg.map { AngleText.degrees(Double($0)) },
            closing: track.closing,
            coasting: track.coasting
        )
    }
}
