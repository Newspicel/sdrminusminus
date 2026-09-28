import CarPlay
import UIKit
import os

final class CarPlaySceneDelegate: UIResponder, CPTemplateApplicationSceneDelegate {
    func templateApplicationScene(
        _ scene: CPTemplateApplicationScene,
        didConnect interfaceController: CPInterfaceController,
        to window: CPWindow
    ) {
        AppRuntime.model.setCarPlay(connected: true)
        window.rootViewController = UIViewController()
        interfaceController.setRootTemplate(CPMapTemplate(), animated: false) { _, error in
            if let error {
                Log.carplay.error("root template: \(error.localizedDescription, privacy: .public)")
            }
        }
        let alert = CPAlertTemplate(
            titleVariants: ["Not built yet"],
            actions: [
                CPAlertAction(title: "OK", style: .cancel) { _ in
                    interfaceController.dismissTemplate(animated: true, completion: nil)
                }
            ]
        )
        interfaceController.presentTemplate(alert, animated: false, completion: nil)
    }

    func templateApplicationScene(
        _ scene: CPTemplateApplicationScene,
        didDisconnect interfaceController: CPInterfaceController,
        from window: CPWindow
    ) {
        AppRuntime.model.setCarPlay(connected: false)
    }
}
