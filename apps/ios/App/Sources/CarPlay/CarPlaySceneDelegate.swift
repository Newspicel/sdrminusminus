import CarPlay
import UIKit

final class CarPlaySceneDelegate: UIResponder, CPTemplateApplicationSceneDelegate {
    private var controller: CarPlayController?

    func templateApplicationScene(
        _ scene: CPTemplateApplicationScene,
        didConnect interfaceController: CPInterfaceController,
        to window: CPWindow
    ) {
        let model = AppRuntime.model
        model.setCarPlay(connected: true)
        let controller = CarPlayController(model: model, interface: interfaceController, window: window)
        self.controller = controller
        controller.styleChanged(interfaceController.carTraitCollection.userInterfaceStyle)
        Task { await controller.start() }
    }

    func templateApplicationScene(
        _ scene: CPTemplateApplicationScene,
        didDisconnect interfaceController: CPInterfaceController,
        from window: CPWindow
    ) {
        controller?.teardown()
        controller = nil
        AppRuntime.model.setCarPlay(connected: false)
    }

    func contentStyleDidChange(_ contentStyle: UIUserInterfaceStyle) {
        controller?.styleChanged(contentStyle)
    }
}
