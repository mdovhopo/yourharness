// The Mac's pairing QR code, scanned in the app (VisionKit's live scanner). Where there's no
// camera (the Simulator), it says so and the link can be pasted on the Macs page instead.

import SwiftUI
import VisionKit

struct ScannerSheet: View {
    @Environment(Fleet.self) private var fleet
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            Group {
                if DataScannerViewController.isSupported && DataScannerViewController.isAvailable {
                    QRScanner { text in
                        guard let link = URL(string: text), link.scheme == "yourharness" else { return }
                        if fleet.pair(link) { dismiss() }
                    }
                    .ignoresSafeArea()
                } else {
                    ContentUnavailableView("No Camera Here", systemImage: "camera", description: Text("Scan the code with another device's camera, or paste the pairing link on the Macs page."))
                }
            }
            .navigationTitle("Scan the Mac's Code")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .cancellationAction) { Button("Cancel", role: .cancel) { dismiss() } } }
            .safeAreaInset(edge: .bottom) {
                Text("On the Mac: YourHarness → Settings → Phone.")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                    .padding(.vertical, 10)
                    .frame(maxWidth: .infinity)
                    .background(.bar)
            }
        }
    }
}

/// The camera, reading QR codes; each payload found goes to `found`.
private struct QRScanner: UIViewControllerRepresentable {
    let found: (String) -> Void

    func makeUIViewController(context: Context) -> DataScannerViewController {
        let scanner = DataScannerViewController(recognizedDataTypes: [.barcode(symbologies: [.qr])], qualityLevel: .balanced, isHighlightingEnabled: true)
        scanner.delegate = context.coordinator
        try? scanner.startScanning()
        return scanner
    }

    func updateUIViewController(_ controller: DataScannerViewController, context: Context) {}

    func makeCoordinator() -> Coordinator { Coordinator(found: found) }

    final class Coordinator: NSObject, DataScannerViewControllerDelegate {
        let found: (String) -> Void
        init(found: @escaping (String) -> Void) { self.found = found }

        func dataScanner(_ scanner: DataScannerViewController, didAdd items: [RecognizedItem], allItems: [RecognizedItem]) {
            for item in items {
                if case .barcode(let code) = item, let text = code.payloadStringValue { found(text) }
            }
        }
    }
}
