import MapKit
import Observation
import SdrmmCore
import SwiftUI

struct MapLayers: Equatable {
    var rays = true
    var heat = true
    var ellipse = true
}

@Observable
final class DfDriveModel {
    private(set) var view: DfView?
    private(set) var pose: PoseView?
    var layers = MapLayers()
    var camera: MapCameraPosition = .userLocation(fallback: .automatic)
    var confirmClear = false
    @ObservationIgnored var showNavigation: (@MainActor () -> Void)?
    @ObservationIgnored private let core: any CoreService
    @ObservationIgnored private let navigation: NavigationModel
    @ObservationIgnored private let report: @MainActor (Error) -> Void

    init(core: any CoreService, navigation: NavigationModel, report: @escaping @MainActor (Error) -> Void) {
        self.core = core
        self.navigation = navigation
        self.report = report
    }

    func apply(_ view: DfView) {
        self.view = view
    }

    func apply(pose: PoseView) {
        self.pose = pose
    }

    func retargeted(_ notice: RetargetNotice) {}
}
