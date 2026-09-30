import SwiftUI
import Vision
import VisionKit
import os

struct QRScannerView: UIViewControllerRepresentable {
    let onPayload: @MainActor (String) -> Void
    let onFailure: @MainActor (String) -> Void

    func makeCoordinator() -> Coordinator {
        Coordinator(onPayload: onPayload)
    }

    func makeUIViewController(context: Context) -> DataScannerViewController {
        let scanner = DataScannerViewController(
            recognizedDataTypes: [.barcode(symbologies: [.qr])],
            qualityLevel: .balanced,
            recognizesMultipleItems: false,
            isHighFrameRateTrackingEnabled: false,
            isHighlightingEnabled: true
        )
        scanner.delegate = context.coordinator
        do {
            try scanner.startScanning()
        } catch {
            Log.pair.error("scanner: \(error.localizedDescription, privacy: .public)")
            let reason = DataScannerViewController.isAvailable ? "Scan failed" : "Camera off"
            Task { @MainActor in onFailure(reason) }
        }
        return scanner
    }

    func updateUIViewController(_ controller: DataScannerViewController, context: Context) {}

    static func dismantleUIViewController(_ controller: DataScannerViewController, coordinator: Coordinator) {
        controller.stopScanning()
    }

    final class Coordinator: NSObject, DataScannerViewControllerDelegate {
        private let onPayload: @MainActor (String) -> Void
        private var delivered = false

        init(onPayload: @escaping @MainActor (String) -> Void) {
            self.onPayload = onPayload
        }

        func dataScanner(
            _ dataScanner: DataScannerViewController,
            didAdd addedItems: [RecognizedItem],
            allItems: [RecognizedItem]
        ) {
            guard !delivered else {
                return
            }
            for item in addedItems {
                guard case .barcode(let barcode) = item, let payload = barcode.payloadStringValue else {
                    continue
                }
                delivered = true
                dataScanner.stopScanning()
                onPayload(payload)
                return
            }
        }
    }
}
