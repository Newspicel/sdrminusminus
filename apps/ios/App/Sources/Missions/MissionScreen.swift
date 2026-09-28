import SdrmmCore
import SwiftUI

struct MissionScreen: View {
    @Environment(AppModel.self) private var model
    let missionID: String

    var body: some View {
        if let mission = model.openMission, mission.id == missionID {
            screen(mission)
                .navigationTitle(mission.title)
                .navigationBarTitleDisplayMode(.inline)
        } else {
            ContentUnavailableView("Mission closed", systemImage: "xmark.circle")
        }
    }

    @ViewBuilder private func screen(_ mission: Mission) -> some View {
        switch mission.kind {
        case .hunt: HuntScreen()
        case .dfDrive: DfDriveView()
        case .radarWatch: RadarWatchView()
        case .survey: SurveyScreen()
        }
    }
}
