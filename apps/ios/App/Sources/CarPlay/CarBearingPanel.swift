import CarPlay
import os

@available(iOS 27.0, *)
final class CarBearingPanel: NSObject, CPMapPanel.Delegate {
    static let wantedItems = 3

    private(set) var visible = false
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

    func panel(_ content: CarPanelContent, units: UnitSystem) -> CPMapPanel {
        let navigate = onNavigate
        let primary = CPTextButton(title: "Navigate", textStyle: .confirm) { _ in navigate() }
        let buttons = CPMapPanelButtonConfiguration(
            primaryAction: primary,
            secondaryButton: nil,
            travelEstimates: Self.estimates(content, units: units)
        )
        let panel = CPMapPanel(title: "DF", sections: sections(content), buttonConfiguration: buttons)
        panel.delegate = self
        return panel
    }

    func update(_ panel: CPMapPanel, with content: CarPanelContent, units: UnitSystem) {
        panel.sections = sections(content)
        panel.buttonConfiguration?.travelEstimates = Self.estimates(content, units: units)
    }

    nonisolated func panelDidShow(_ panel: CPMapPanel) {
        Task { @MainActor [weak self] in self?.visible = true }
    }

    nonisolated func panelDidHide(_ panel: CPMapPanel) {
        Task { @MainActor [weak self] in self?.visible = false }
    }

    private func sections(_ content: CarPanelContent) -> [CPMapPanelSection] {
        var items = [
            CPMapPanelItem(listItem: CPListItem(text: content.bearing, detailText: content.bearingDetail)),
            CPMapPanelItem(listItem: CPListItem(text: content.guidance, detailText: content.guidanceDetail)),
        ]
        let buttons = grid(content)
        if buttons.isEmpty {
            return [CPMapPanelSection(title: nil, items: items)]
        }
        if CPPanel.maximumPanelItemsCount >= Self.wantedItems {
            items.append(CPMapPanelItem(gridButtons: buttons))
        } else {
            Log.carplay.info("panel holds \(CPPanel.maximumPanelItemsCount) items, grid dropped")
        }
        return [CPMapPanelSection(title: nil, items: items)]
    }

    private func grid(_ content: CarPanelContent) -> [CPGridButton] {
        let calibrate = onCalibrate
        let clear = onClear
        var buttons: [CPGridButton] = []
        if content.canCalibrate {
            buttons.append(
                CPGridButton(titleVariants: ["Calibrate"], image: Self.symbol("dot.scope")) { _ in calibrate()
                }
            )
        }
        if content.canClear {
            buttons.append(
                CPGridButton(titleVariants: ["Clear"], image: Self.symbol("trash")) { _ in clear() }
            )
        }
        return buttons
    }

    private static func symbol(_ name: String) -> UIImage {
        UIImage(systemName: name) ?? UIImage()
    }

    private static func estimates(_ content: CarPanelContent, units: UnitSystem) -> CPTravelEstimates {
        CPTravelEstimates(
            distanceRemaining: CarUnits.measure(content.targetDistanceM ?? 0, units),
            timeRemaining: 0
        )
    }
}

nonisolated enum CarUnits {
    static func measure(_ meters: Double, _ units: UnitSystem) -> Measurement<UnitLength> {
        let base = Measurement(value: max(0, meters), unit: UnitLength.meters)
        switch units {
        case .metric: return meters < 1_000 ? base : base.converted(to: .kilometers)
        case .imperial: return meters < 305 ? base.converted(to: .feet) : base.converted(to: .miles)
        }
    }
}
