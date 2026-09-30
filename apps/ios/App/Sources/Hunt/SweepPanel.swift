import SdrmmCore
import SwiftUI

nonisolated struct SweepPetal: Equatable, Sendable {
    let startDeg: Double
    let endDeg: Double
    let length: Double
}

nonisolated enum SweepPlot {
    static func binWidth(count: Int) -> Double {
        count > 0 ? 360 / Double(count) : 360
    }

    static func screenDeg(bearing: Double, up: Double) -> Double {
        let turn = (bearing - up).truncatingRemainder(dividingBy: 360)
        return turn < 0 ? turn + 360 : turn
    }

    static func petals(bins: some Collection<UInt8>, up: Double) -> [SweepPetal] {
        let width = binWidth(count: bins.count)
        return bins.enumerated().compactMap { index, level in
            guard level > 0 else {
                return nil
            }
            let start = screenDeg(bearing: Double(index) * width, up: up)
            return SweepPetal(startDeg: start, endDeg: start + width, length: Double(level) / 255)
        }
    }

    static func point(center: CGPoint, radius: Double, screenDeg: Double) -> CGPoint {
        let radians = (screenDeg - 90) * .pi / 180
        return CGPoint(x: center.x + radius * cos(radians), y: center.y + radius * sin(radians))
    }
}

nonisolated enum SweepText {
    static func phase(_ phase: SweepPhase) -> String? {
        switch phase {
        case .off: nil
        case .idle: "Turn slowly"
        case .sweeping: "Sweeping"
        case .noHeading: "No heading"
        case .shortSpan: "Short"
        case .lowContrast: "Low contrast"
        case .poorFit: "Poor fit"
        case .headingPoor: "Heading poor"
        case .tooFast: "Too fast"
        case .done: "Done"
        }
    }

    static func problem(_ phase: SweepPhase) -> Bool {
        switch phase {
        case .noHeading, .shortSpan, .lowContrast, .poorFit, .headingPoor, .tooFast: true
        case .off, .idle, .sweeping, .done: false
        }
    }

    static func sigma(_ degrees: Float?) -> String {
        guard let degrees, degrees.isFinite else {
            return "-"
        }
        return "\u{00B1}\(Int(degrees.rounded()))\u{00B0}"
    }

    static func covered(_ degrees: Float) -> String {
        guard degrees.isFinite else {
            return "-"
        }
        return "\(Int(degrees.rounded()))\u{00B0}"
    }
}

struct SweepPanel: View {
    @Environment(AppModel.self) private var model
    let controls: [MissionControl]

    var body: some View {
        let hunt = model.hunt
        VStack(spacing: 12) {
            if let sweep = hunt.view?.sweep, sweep.phase != .off {
                SweepRose(sweep: sweep, heading: model.pose?.headingDeg)
                    .frame(maxWidth: 220, maxHeight: 220)
                readouts(sweep)
            }
            HStack {
                if controls.contains(.sweep) {
                    Button(hunt.sweeping ? "Stop sweep" : "Sweep") { Task { await hunt.toggleSweep() } }
                        .buttonStyle(.bordered)
                        .disabled(hunt.busy)
                        .accessibilityHint("Turn slowly all the way round")
                        .accessibilityIdentifier(A11y.huntSweep)
                }
                if controls.contains(.mark) {
                    Button("Mark") { Task { await hunt.mark() } }
                        .buttonStyle(.bordered)
                        .disabled(hunt.busy)
                        .accessibilityHint("Send the current heading as a bearing")
                        .accessibilityIdentifier(A11y.huntMark)
                }
            }
        }
    }

    private func readouts(_ sweep: SweepView) -> some View {
        HStack(spacing: 16) {
            LabeledReadout(
                title: "Peak",
                value: sweep.peakDeg.map { AngleText.degrees(Double($0)) } ?? "-",
                id: A11y.huntSweepPeak
            )
            LabeledReadout(title: "Error", value: SweepText.sigma(sweep.sigmaDeg), id: A11y.huntSweepSigma)
            LabeledReadout(
                title: "Covered",
                value: SweepText.covered(sweep.coveredDeg),
                id: A11y.huntSweepCovered
            )
            if let label = SweepText.phase(sweep.phase) {
                Chip(text: label, color: SweepText.problem(sweep.phase) ? Palette.warn : Palette.accent)
                    .accessibilityIdentifier(A11y.huntSweepPhase)
            }
        }
    }
}

private struct LabeledReadout: View {
    let title: String
    let value: String
    let id: String

    var body: some View {
        VStack(spacing: 2) {
            Text(title)
                .font(.caption)
                .foregroundStyle(.secondary)
                .accessibilityHidden(true)
            Text(value)
                .font(.body.monospacedDigit())
                .accessibilityLabel(value)
                .accessibilityIdentifier(id)
        }
    }
}

struct SweepRose: View {
    let sweep: SweepView
    let heading: Double?

    var body: some View {
        Canvas { context, size in
            let radius = min(size.width, size.height) / 2 - 12
            let center = CGPoint(x: size.width / 2, y: size.height / 2)
            let up = heading ?? 0
            drawPetals(context, center: center, radius: radius, up: up)
            drawRing(context, center: center, radius: radius, up: up)
            if let peak = sweep.peakDeg {
                let tip = SweepPlot.point(
                    center: center,
                    radius: radius,
                    screenDeg: SweepPlot.screenDeg(bearing: Double(peak), up: up)
                )
                var needle = Path()
                needle.move(to: center)
                needle.addLine(to: tip)
                context.stroke(needle, with: .color(Palette.danger), lineWidth: 3)
            }
        }
        .aspectRatio(1, contentMode: .fit)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(sweep.peakDeg.map { "Peak \(AngleText.degrees(Double($0)))" } ?? "No peak")
        .accessibilityIdentifier(A11y.huntSweepRose)
    }

    private func drawPetals(_ context: GraphicsContext, center: CGPoint, radius: Double, up: Double) {
        for petal in SweepPlot.petals(bins: sweep.bins, up: up) {
            var wedge = Path()
            wedge.move(to: center)
            wedge.addArc(
                center: center,
                radius: radius * petal.length,
                startAngle: .degrees(petal.startDeg - 90),
                endAngle: .degrees(petal.endDeg - 90),
                clockwise: false
            )
            wedge.closeSubpath()
            context.fill(wedge, with: .color(Palette.accent.opacity(0.35 + 0.5 * petal.length)))
        }
    }

    private func drawRing(_ context: GraphicsContext, center: CGPoint, radius: Double, up: Double) {
        let ring = Path(
            ellipseIn: CGRect(
                x: center.x - radius,
                y: center.y - radius,
                width: 2 * radius,
                height: 2 * radius
            )
        )
        context.stroke(ring, with: .color(.secondary), lineWidth: 1)
        let north = SweepPlot.point(
            center: center,
            radius: radius + 6,
            screenDeg: SweepPlot.screenDeg(bearing: 0, up: up)
        )
        context.draw(Text("N").font(.caption2.bold()), at: north)
        if heading != nil {
            var tick = Path()
            tick.move(to: SweepPlot.point(center: center, radius: radius - 8, screenDeg: 0))
            tick.addLine(to: SweepPlot.point(center: center, radius: radius + 2, screenDeg: 0))
            context.stroke(tick, with: .color(.primary), lineWidth: 2)
        }
    }
}
