import Observation
import SdrmmCore

@Observable
final class SurveyModel {
    private(set) var view: SurveyView?
    private(set) var points: [SurveyPoint] = []

    func apply(_ view: SurveyView) {
        self.view = view
    }

    func append(_ points: [SurveyPoint]) {
        self.points.append(contentsOf: points)
    }

    func clearTrail() {
        points = []
    }
}
