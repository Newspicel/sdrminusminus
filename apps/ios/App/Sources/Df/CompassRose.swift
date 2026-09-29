import SdrmmCore
import SwiftUI

nonisolated struct RoseState: Equatable {
    var bearingDeg: Double?
    var sigmaDeg: Double?
    var guidanceDeg: Double?
    var northDeg: Double
    var headingUp: Bool

    static func make(view: DfView?, pose: PoseView?) -> RoseState {
        let live = view?.state == .live
        let sigma = live ? view?.sigmaDeg.map(Double.init) : nil
        if let heading = pose?.headingDeg, let relative = view?.bearingRelDeg {
            return RoseState(
                bearingDeg: live ? Double(relative) : nil,
                sigmaDeg: sigma,
                guidanceDeg: view?.guidance?.headingRelDeg,
                northDeg: normalized(360 - heading),
                headingUp: true
            )
        }
        return RoseState(
            bearingDeg: live ? view?.bearingTrueDeg.map(Double.init) : nil,
            sigmaDeg: sigma,
            guidanceDeg: view?.guidance?.headingTrueDeg,
            northDeg: 0,
            headingUp: false
        )
    }

    static func normalized(_ degrees: Double) -> Double {
        guard degrees.isFinite else {
            return 0
        }
        let turn = degrees.truncatingRemainder(dividingBy: 360)
        return turn < 0 ? turn + 360 : turn
    }

    var sideText: String? {
        guard let bearingDeg else {
            return nil
        }
        guard headingUp else {
            return "North up"
        }
        let turn = Self.normalized(bearingDeg)
        let rounded = Int(turn.rounded()) % 360
        if rounded == 0 {
            return "Ahead"
        }
        return rounded <= 180 ? "\(rounded)\u{00B0} right" : "\(360 - rounded)\u{00B0} left"
    }
}

struct CompassRose: View {
    let state: RoseState
    var trueBearingDeg: Double?

    var body: some View {
        Canvas { context, size in
            RoseDrawing(state: state, size: size).draw(in: &context)
        }
        .aspectRatio(1, contentMode: .fit)
        .accessibilityElement()
        .accessibilityLabel(label)
        .accessibilityValue(state.sideText ?? "")
    }

    private var label: String {
        guard let bearing = trueBearingDeg ?? state.bearingDeg, state.bearingDeg != nil else {
            return "No bearing"
        }
        return "Bearing \(AngleText.degrees(bearing))"
    }
}

private struct RoseDrawing {
    let state: RoseState
    let size: CGSize

    private static let compactSide: CGFloat = 120

    private var center: CGPoint { CGPoint(x: size.width / 2, y: size.height / 2) }
    private var compact: Bool { min(size.width, size.height) < Self.compactSide }
    private var radius: CGFloat { max(1, min(size.width, size.height) / 2 - (compact ? 4 : 14)) }

    func draw(in context: inout GraphicsContext) {
        ring(&context)
        ticks(&context)
        letters(&context)
        if state.headingUp, !compact {
            car(&context)
        }
        if let bearing = state.bearingDeg {
            wedge(&context, bearing: bearing)
            needle(&context, bearing: bearing)
        }
        if let guidance = state.guidanceDeg {
            arrow(&context, toward: guidance)
        }
        let dot = CGRect(x: center.x - 3, y: center.y - 3, width: 6, height: 6)
        context.fill(Path(ellipseIn: dot), with: .color(.primary))
    }

    private func point(_ degrees: Double, _ distance: CGFloat) -> CGPoint {
        let theta = degrees * .pi / 180
        return CGPoint(x: center.x + distance * sin(theta), y: center.y - distance * cos(theta))
    }

    private func ring(_ context: inout GraphicsContext) {
        let rect = CGRect(x: center.x - radius, y: center.y - radius, width: 2 * radius, height: 2 * radius)
        context.stroke(Path(ellipseIn: rect), with: .color(.secondary), lineWidth: 1.5)
    }

    private func ticks(_ context: inout GraphicsContext) {
        for step in 0..<12 {
            let angle = state.northDeg + Double(step) * 30
            let cardinal = step.isMultiple(of: 3)
            var path = Path()
            path.move(to: point(angle, radius))
            path.addLine(to: point(angle, radius * (cardinal ? 0.84 : 0.92)))
            context.stroke(path, with: .color(.secondary), lineWidth: cardinal ? 2 : 1)
        }
    }

    private func letters(_ context: inout GraphicsContext) {
        for (index, letter) in (compact ? ["N"] : ["N", "E", "S", "W"]).enumerated() {
            let angle = state.northDeg + Double(index) * 90
            let text = Text(letter).font(.caption.bold()).foregroundStyle(
                index == 0 ? Color.primary : .secondary
            )
            context.draw(text, at: point(angle, radius * 0.74))
        }
    }

    private func car(_ context: inout GraphicsContext) {
        let glyph = Text(Image(systemName: "car.fill")).font(.caption2).foregroundStyle(.secondary)
        context.draw(glyph, at: point(0, radius + 8))
    }

    private func wedge(_ context: inout GraphicsContext, bearing: Double) {
        guard let sigma = state.sigmaDeg, sigma > 0 else {
            return
        }
        var path = Path()
        path.move(to: center)
        path.addArc(
            center: center,
            radius: radius,
            startAngle: .degrees(bearing - sigma - 90),
            endAngle: .degrees(bearing + sigma - 90),
            clockwise: false
        )
        path.closeSubpath()
        context.fill(path, with: .color(Palette.accent.opacity(0.2)))
    }

    private func needle(_ context: inout GraphicsContext, bearing: Double) {
        var path = Path()
        path.move(to: center)
        path.addLine(to: point(bearing, radius * 0.92))
        context.stroke(path, with: .color(Palette.accent), style: StrokeStyle(lineWidth: 3, lineCap: .round))
    }

    private func arrow(_ context: inout GraphicsContext, toward degrees: Double) {
        var path = Path()
        path.move(to: center)
        path.addLine(to: point(degrees, radius * 0.7))
        context.stroke(path, with: .color(Palette.warn), style: StrokeStyle(lineWidth: 6, lineCap: .round))
    }
}
