import SwiftUI

@main
struct SdrmmApp: App {
    @UIApplicationDelegateAdaptor(AppDelegate.self) private var delegate
    @Environment(\.scenePhase) private var phase

    var body: some Scene {
        WindowGroup {
            RootView(demo: AppRuntime.demo)
                .environment(AppRuntime.model)
                .task { AppRuntime.start() }
                .onOpenURL { AppRuntime.model.openLink($0) }
        }
        .onChange(of: phase) { _, phase in
            AppRuntime.model.scene(phase)
        }
    }
}
