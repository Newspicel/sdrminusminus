import CoreGraphics
import Foundation
import SdrmmCore

nonisolated enum RadarImageError: Error, Equatable {
    case badSize(expected: Int, actual: Int)
    case emptyImage
    case cgFailure

    var kind: String {
        switch self {
        case .badSize: "bad size"
        case .emptyImage: "empty"
        case .cgFailure: "CoreGraphics"
        }
    }
}

nonisolated enum RadarImageRenderer {
    static func cgImage(_ image: RgbaImage) throws(RadarImageError) -> CGImage {
        let width = Int(image.width)
        let height = Int(image.height)
        guard width > 0, height > 0 else {
            throw .emptyImage
        }
        let expected = width * height * 4
        guard image.rgba.count == expected else {
            throw .badSize(expected: expected, actual: image.rgba.count)
        }
        guard let provider = CGDataProvider(data: image.rgba as CFData),
            let space = CGColorSpace(name: CGColorSpace.sRGB),
            let rendered = CGImage(
                width: width,
                height: height,
                bitsPerComponent: 8,
                bitsPerPixel: 32,
                bytesPerRow: width * 4,
                space: space,
                bitmapInfo: CGBitmapInfo(
                    rawValue: CGImageAlphaInfo.premultipliedLast.rawValue
                        | CGBitmapInfo.byteOrder32Big.rawValue
                ),
                provider: provider,
                decode: nil,
                shouldInterpolate: false,
                intent: .defaultIntent
            )
        else {
            throw .cgFailure
        }
        return rendered
    }
}
