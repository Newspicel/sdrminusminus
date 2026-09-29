import CarPlay
import SdrmmCore

nonisolated struct CarPanelContent: Equatable, Sendable {
    let bearing: String
    let bearingDetail: String
    let confidence: String?
    let guidance: String
    let guidanceDetail: String
    let canNavigate: Bool
    let targetDistanceM: Double?

    static func make(df: DfView?, pose: PoseView?, here: LatLon?, units: UnitSystem) -> CarPanelContent {
        let rose = RoseState.make(view: df, pose: pose)
        let confidence = DfText.confidence(df)
        let detail: String
        if let confidence {
            detail = "Bearing \(confidence)"
        } else {
            detail = df.flatMap { DfText.state($0.state) } ?? "Waiting"
        }
        let guidance = df?.guidance
        return CarPanelContent(
            bearing: rose.bearingDeg.map(AngleText.degrees) ?? "-",
            bearingDetail: detail,
            confidence: confidence,
            guidance: guidance.map { AngleText.degrees($0.headingTrueDeg) } ?? "-",
            guidanceDetail: guidanceDetail(guidance, units: units),
            canNavigate: df?.target != nil,
            targetDistanceM: zip(here, df?.target).map { geoDistanceM(from: $0, to: $1.at) }
        )
    }

    static func guidanceDetail(_ guidance: GuidanceView?, units: UnitSystem) -> String {
        guard let guidance else {
            return "No guidance"
        }
        let distance = DistanceText.short(guidance.distanceM, units)
        switch guidance.kind {
        case .probe: return "Cross \(distance)"
        case .estimate: return "Approach \(distance)"
        }
    }

    var infoBearing: String {
        guard let confidence else {
            return "\(bearing) \(bearingDetail)"
        }
        return "\(bearing) \(confidence)"
    }

    private static func zip<A, B>(_ first: A?, _ second: B?) -> (A, B)? {
        guard let first, let second else {
            return nil
        }
        return (first, second)
    }
}

final class CarDfInfo {
    private let onNavigate: @MainActor () -> Void
    private let onCalibrate: @MainActor () -> Void
    private let onClear: @MainActor () -> Void

    init(
        onNavigate: @escaping @MainActor () -> Void,
        onCalibrate: @escaping @MainActor () -> Void,
        onClear: @escaping @MainActor () -> Void
    ) {
        self.onNavigate = onNavigate
        self.onCalibrate = onCalibrate
        self.onClear = onClear
    }

    func template(_ content: CarPanelContent) -> CPInformationTemplate {
        CPInformationTemplate(title: "DF", layout: .leading, items: items(content), actions: actions(content))
    }

    func update(_ template: CPInformationTemplate, with content: CarPanelContent) {
        template.items = items(content)
        template.actions = actions(content)
    }

    private func items(_ content: CarPanelContent) -> [CPInformationItem] {
        [
            CPInformationItem(title: "Bearing", detail: content.infoBearing),
            CPInformationItem(title: "Guidance", detail: content.guidanceDetail),
        ]
    }

    private func actions(_ content: CarPanelContent) -> [CPTextButton] {
        let calibrate = onCalibrate
        let clear = onClear
        let navigate = onNavigate
        var buttons = [
            CPTextButton(title: "Calibrate", textStyle: .normal) { _ in calibrate() },
            CPTextButton(title: "Clear", textStyle: .cancel) { _ in clear() },
        ]
        if content.canNavigate {
            buttons.append(CPTextButton(title: "Navigate", textStyle: .confirm) { _ in navigate() })
        }
        return buttons
    }
}
