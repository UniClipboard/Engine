import SwiftUI

@main
struct EngineProbeApp: App {
    @State private var model = ProbeModel()
    @Environment(\.scenePhase) private var scenePhase

    var body: some Scene {
        WindowGroup {
            ProbeView(model: model)
                .task {
                    await model.startIfNeeded()
                    await model.runAutomatedAction()
                }
                .onOpenURL { url in
                    Task { await model.handleCommandURL(url) }
                }
                .onChange(of: scenePhase) { _, phase in
                    Task { await model.handleScenePhase(phase) }
                }
        }
    }
}
