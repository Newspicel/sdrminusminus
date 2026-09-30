import Foundation

nonisolated enum DistanceText {
    private struct Unit {
        let short: String
        let singular: String
        let plural: String
    }

    private static let feetPerMeter = 3.28084
    private static let metersPerMile = 1609.344
    private static let meter = Unit(short: "m", singular: "meter", plural: "meters")
    private static let kilometer = Unit(short: "km", singular: "kilometer", plural: "kilometers")
    private static let foot = Unit(short: "ft", singular: "foot", plural: "feet")
    private static let mile = Unit(short: "mi", singular: "mile", plural: "miles")

    static func short(_ meters: Double, _ units: UnitSystem) -> String {
        guard let (number, unit) = amount(meters, units) else {
            return "-"
        }
        return "\(number) \(unit.short)"
    }

    static func spoken(_ meters: Double, _ units: UnitSystem) -> String {
        guard let (number, unit) = amount(meters, units) else {
            return "-"
        }
        let trimmed = number.hasSuffix(".0") ? String(number.dropLast(2)) : number
        return "\(trimmed) \(trimmed == "1" ? unit.singular : unit.plural)"
    }

    private static func amount(_ meters: Double, _ units: UnitSystem) -> (String, Unit)? {
        guard meters.isFinite else {
            return nil
        }
        switch units {
        case .metric:
            if meters < 100 {
                return (rounded(meters, to: 5), meter)
            }
            if meters < 1_000 {
                return (rounded(meters, to: 10), meter)
            }
            return (decimal(meters / 1_000), kilometer)
        case .imperial:
            let feet = meters * feetPerMeter
            if feet < 1_000 {
                return (rounded(feet, to: 50), foot)
            }
            return (decimal(meters / metersPerMile), mile)
        }
    }

    private static func decimal(_ value: Double) -> String {
        String(format: value < 10 ? "%.1f" : "%.0f", value)
    }

    private static func rounded(_ value: Double, to step: Double) -> String {
        String(Int((value / step).rounded() * step))
    }
}

nonisolated enum FrequencyText {
    static func text(_ hz: Double) -> String {
        guard hz.isFinite else {
            return "-"
        }
        if hz >= 1e9 {
            return String(format: "%.4f GHz", hz / 1e9)
        }
        if hz >= 1e6 {
            return String(format: "%.3f MHz", hz / 1e6)
        }
        if hz >= 1e3 {
            return String(format: "%.1f kHz", hz / 1e3)
        }
        return String(format: "%.0f Hz", hz)
    }
}

nonisolated enum AngleText {
    static func degrees(_ value: Double) -> String {
        guard value.isFinite else {
            return "-"
        }
        let turn = value.truncatingRemainder(dividingBy: 360)
        let whole = Int((turn < 0 ? turn + 360 : turn).rounded()) % 360
        return String(format: "%03d\u{00B0}", whole)
    }
}

nonisolated enum DurationText {
    private static let longest: Double = 100 * 24 * 3_600

    static func text(_ seconds: Double) -> String {
        guard seconds.isFinite, seconds < longest else {
            return "-"
        }
        if seconds < 60 {
            return "<1 min"
        }
        let minutes = Int((seconds / 60).rounded())
        if minutes < 60 {
            return "\(minutes) min"
        }
        return String(format: "%d h %02d", minutes / 60, minutes % 60)
    }
}

nonisolated enum LevelText {
    static func db(_ value: Float?) -> String {
        guard let value, value.isFinite else {
            return "-"
        }
        return String(format: "%.1f dB", value)
    }
}
