import SdrmmCore
import SwiftUI
import UIKit

struct RootView: View {
    let demo: DemoSession

    var body: some View {
        if let failure = AppRuntime.coreFailure {
            CoreFailedView(detail: failure)
        } else if let model = demo.model {
            ShellView()
                .environment(model)
                .task { await model.run() }
                .id(ObjectIdentifier(model))
        } else {
            ShellView()
        }
    }
}

struct ShellView: View {
    @Environment(AppModel.self) private var model
    @State private var details: Banner?

    var body: some View {
        @Bindable var model = model
        content
            .overlay(alignment: .top) {
                if let banner = model.banner {
                    BannerView(banner: banner) { details = banner }
                        .padding(.horizontal)
                        .task(id: banner.id) { await expire(banner) }
                }
            }
            .sheet(isPresented: $model.showSettings) {
                SettingsView().environment(model)
            }
            .sheet(isPresented: $model.showPairSheet) {
                PairView().environment(model)
            }
            .sheet(item: $details) { banner in
                BannerDetails(banner: banner)
            }
    }

    @ViewBuilder private var content: some View {
        @Bindable var model = model
        if model.needsPairing {
            PairView()
        } else {
            NavigationStack(path: $model.path) {
                MissionListView()
                    .navigationDestination(for: Screen.self) { screen in
                        switch screen {
                        case .mission(let id): MissionScreen(missionID: id)
                        case .navigation: NavigationScreen()
                        }
                    }
            }
        }
    }

    private func expire(_ banner: Banner) async {
        do {
            try await Task.sleep(for: .seconds(banner.seconds))
        } catch {
            return
        }
        if model.banner == banner {
            model.dismissBanner()
        }
    }
}

struct CoreFailedView: View {
    let detail: String

    var body: some View {
        ContentUnavailableView {
            Label("Core failed", systemImage: "exclamationmark.triangle")
        } description: {
            Text(detail)
        } actions: {
            Button("Copy") { UIPasteboard.general.string = detail }
        }
    }
}

private struct BannerDetails: View {
    let banner: Banner
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            ScrollView {
                Text(fullText)
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding()
            }
            .navigationTitle("Details")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Close") { dismiss() }
                }
                ToolbarItem(placement: .primaryAction) {
                    Button("Copy") { UIPasteboard.general.string = fullText }
                }
            }
        }
        .presentationDetents([.medium])
    }

    private var fullText: String {
        guard let detail = banner.detail, detail != banner.text else {
            return banner.text
        }
        return "\(banner.text)\n\(detail)"
    }
}
