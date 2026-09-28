import CoreGraphics
import Observation
import SdrmmCore

@Observable
final class RadarModel {
    private(set) var view: RadarView?
    private(set) var image: CGImage?
    private(set) var imageFailed = false

    func apply(_ view: RadarView) {
        self.view = view
    }

    func apply(image: RgbaImage) {}
}
