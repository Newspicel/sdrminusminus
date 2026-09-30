import SdrmmCore
import SwiftUI

struct MissionListView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        @Bindable var model = model
        List {
            ForEach(model.groupedMissions, id: \.0) { kind, missions in
                Section {
                    ForEach(missions, id: \.id) { mission in
                        MissionRow(mission: mission)
                    }
                } header: {
                    Text(Self.header(kind)).textCase(nil)
                }
            }
        }
        .overlay {
            if model.groupedMissions.isEmpty {
                Text("No missions")
                    .foregroundStyle(.secondary)
                    .accessibilityIdentifier(A11y.missionsEmpty)
            }
        }
        .refreshable { await model.refresh() }
        .navigationTitle(model.activeServer?.name ?? "Missions")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar { toolbar }
        .confirmationDialog(
            "Switch workspace?",
            isPresented: confirming,
            titleVisibility: .visible,
            presenting: model.confirmWorkspace
        ) { workspace in
            Button("Switch") { Task { await model.switchWorkspace(workspace) } }
            Button("Cancel", role: .cancel) { model.confirmWorkspace = nil }
        }
    }

    @ToolbarContentBuilder private var toolbar: some ToolbarContent {
        ToolbarItem(placement: .topBarLeading) {
            LinkStatus()
        }
        ToolbarItem(placement: .principal) {
            VStack(spacing: 0) {
                Text(model.activeServer?.name ?? "Missions")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                workspaceMenu
            }
        }
        ToolbarItem(placement: .topBarTrailing) {
            if let exit = model.demoExit {
                Button("Leave demo") { exit() }
                    .accessibilityIdentifier(A11y.missionsLeaveDemo)
            } else {
                Button("Settings", systemImage: "gearshape") { model.showSettings = true }
                    .accessibilityIdentifier(A11y.missionsSettings)
            }
        }
    }

    @ViewBuilder private var workspaceMenu: some View {
        if let view = model.missions {
            Menu {
                ForEach(view.workspaces.filter { $0.id != view.workspace.id }, id: \.id) { workspace in
                    Button(workspace.name) { model.confirmWorkspace = workspace }
                }
            } label: {
                Label(view.workspace.name, systemImage: "chevron.down")
                    .labelStyle(.titleAndIcon)
                    .font(.headline)
            }
            .accessibilityIdentifier(A11y.missionsWorkspace)
        }
    }

    private var confirming: Binding<Bool> {
        Binding(
            get: { model.confirmWorkspace != nil },
            set: { shown in
                if !shown { model.confirmWorkspace = nil }
            }
        )
    }

    static func header(_ kind: MissionKind) -> String {
        switch kind {
        case .hunt: "Hunt"
        case .dfDrive: "DF drive"
        case .radarWatch: "Radar"
        case .survey: "Survey"
        }
    }
}

private struct MissionRow: View {
    @Environment(AppModel.self) private var model
    let mission: Mission

    var body: some View {
        Button {
            model.open(mission)
        } label: {
            HStack {
                VStack(alignment: .leading, spacing: 2) {
                    Text(mission.title)
                    Text(mission.detail)
                        .font(.footnote.monospacedDigit())
                        .foregroundStyle(.secondary)
                }
                Spacer()
                if let blocker = mission.blocker, !mission.ready {
                    Text(blocker)
                        .font(.footnote)
                        .foregroundStyle(Palette.warn)
                }
            }
            .opacity(mission.ready ? 1 : 0.5)
        }
        .foregroundStyle(.primary)
        .accessibilityIdentifier(A11y.missionRow(mission.id))
    }
}

private struct LinkStatus: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Button {
            if case .offline = model.link {
                Task { await model.reconnect() }
            }
        } label: {
            HStack(spacing: 6) {
                Circle().fill(color).frame(width: 8, height: 8)
                Text(label).font(.footnote)
            }
        }
        .foregroundStyle(.primary)
        .accessibilityIdentifier(A11y.missionsLink)
    }

    private var label: String {
        switch model.link {
        case .online: "Online"
        case .connecting: "Connecting"
        case .offline: "Offline"
        case .refused: "Refused"
        }
    }

    private var color: Color {
        switch model.link {
        case .online: .green
        case .connecting: Palette.warn
        case .offline: .secondary
        case .refused: Palette.danger
        }
    }
}
