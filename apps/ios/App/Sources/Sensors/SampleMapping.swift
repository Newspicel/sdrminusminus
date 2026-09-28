import CoreLocation
import CoreMotion
import SdrmmCore

nonisolated enum SampleMapping {
    static func location(_ location: CLLocation) -> LocationSample? {
        guard location.horizontalAccuracy >= 0 else {
            return nil
        }
        let vertical = valid(location.verticalAccuracy)
        return LocationSample(
            tUnixMs: millis(location.timestamp),
            lat: location.coordinate.latitude,
            lon: location.coordinate.longitude,
            altM: vertical == nil ? nil : location.altitude,
            hAccM: location.horizontalAccuracy,
            vAccM: vertical,
            speedMps: valid(location.speed),
            speedAccMps: valid(location.speedAccuracy),
            courseDeg: valid(location.course),
            courseAccDeg: valid(location.courseAccuracy)
        )
    }

    static func heading(_ heading: CLHeading) -> HeadingSample {
        self.heading(
            trueDeg: heading.trueHeading,
            magneticDeg: heading.magneticHeading,
            accuracyDeg: heading.headingAccuracy,
            at: heading.timestamp
        )
    }

    static func heading(trueDeg: Double, magneticDeg: Double, accuracyDeg: Double, at time: Date)
        -> HeadingSample
    {
        HeadingSample(
            tUnixMs: millis(time),
            trueDeg: valid(trueDeg),
            magneticDeg: magneticDeg,
            accuracyDeg: valid(accuracyDeg)
        )
    }

    static func motion(
        _ motion: CMDeviceMotion,
        frame: MotionFrame,
        uptimeNow: TimeInterval,
        wallNow: Date
    ) -> MotionSample {
        let attitude = motion.attitude.quaternion
        return MotionSample(
            tUnixMs: unixMillis(uptime: motion.timestamp, uptimeNow: uptimeNow, wallNow: wallNow),
            frame: frame,
            qw: attitude.w,
            qx: attitude.x,
            qy: attitude.y,
            qz: attitude.z,
            rotX: motion.rotationRate.x,
            rotY: motion.rotationRate.y,
            rotZ: motion.rotationRate.z,
            gravX: motion.gravity.x,
            gravY: motion.gravity.y,
            gravZ: motion.gravity.z,
            headingDeg: valid(motion.heading),
            magAccuracy: magAccuracy(motion.magneticField.accuracy)
        )
    }

    static func magAccuracy(_ accuracy: CMMagneticFieldCalibrationAccuracy) -> MagAccuracy {
        switch accuracy {
        case .low: .low
        case .medium: .medium
        case .high: .high
        case .uncalibrated: .uncalibrated
        @unknown default: .uncalibrated
        }
    }

    static func unixMillis(uptime: TimeInterval, uptimeNow: TimeInterval, wallNow: Date) -> Int64 {
        Int64(((wallNow.timeIntervalSince1970 - (uptimeNow - uptime)) * 1000).rounded())
    }

    private static func millis(_ date: Date) -> Int64 {
        Int64((date.timeIntervalSince1970 * 1000).rounded())
    }

    private static func valid(_ value: Double) -> Double? {
        value < 0 || !value.isFinite ? nil : value
    }
}
