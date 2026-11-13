import CoreVideo
import Foundation
import Vision

/// Finds the person's face and hand gestures in the captured frames, for face tracking and
/// gesture control.
///
/// The frames are the ones the encoder gets, already rotated and mirrored by the capture
/// connection, so positions are in the picture the host receives. Analysis runs on its own
/// queue, at most `GestureTuning.analysisInterval` apart, and a frame is skipped while the
/// previous one is still being analysed, so it never holds up capture or encoding.
final class VisionAnalyzer {
    struct Options: Equatable {
        var faceTracking = false
        var gestures = false

        var isActive: Bool {
            faceTracking || gestures
        }
    }

    /// The largest face, or `nil` once when the face is lost. Called on the analysis queue.
    var onFace: ((PictureRect?) -> Void)?
    /// A gesture fired, began, changed or ended, with where the open palm was for `palm`.
    /// Called on the analysis queue.
    var onGesture: ((HandGestureRecognizer.Event, PicturePoint?) -> Void)?

    private static let maximumHandCount = 2

    private let queue = DispatchQueue(label: "app.paircam.vision", qos: .userInitiated)

    // Shared between the video queue and the analysis queue.
    private let lock = NSLock()
    private var options = Options()
    private var isBusy = false
    private var lastSubmitted: TimeInterval = 0

    // Analysis queue state.
    private let faceRequest = VNDetectFaceRectanglesRequest()
    private let handRequest: VNDetectHumanHandPoseRequest = {
        let request = VNDetectHumanHandPoseRequest()
        request.maximumHandCount = VisionAnalyzer.maximumHandCount
        return request
    }()
    private var recognizer = HandGestureRecognizer()
    private var faceVisible = false
    private var failures = 0

    /// Turns face tracking and gesture recognition on or off. Any thread.
    func setOptions(_ newOptions: Options) {
        lock.lock()
        options = newOptions
        lock.unlock()

        queue.async {
            if !newOptions.faceTracking {
                self.faceVisible = false
            }
            if !newOptions.gestures {
                for event in self.recognizer.reset() {
                    self.onGesture?(event, nil)
                }
            }
        }
    }

    /// Offers a captured frame. Returns at once; the frame is analysed only if something is on,
    /// enough time has passed and the previous analysis has finished. Call on the video queue.
    func submit(_ pixelBuffer: CVPixelBuffer) {
        let now = monotonicNow()
        lock.lock()
        let current = options
        guard
            current.isActive,
            !isBusy,
            now - lastSubmitted >= GestureTuning.analysisInterval
        else {
            lock.unlock()
            return
        }
        isBusy = true
        lastSubmitted = now
        lock.unlock()

        queue.async {
            self.analyze(pixelBuffer, options: current, time: now)
            self.lock.lock()
            self.isBusy = false
            self.lock.unlock()
        }
    }

    // MARK: - Analysis queue

    private func analyze(_ pixelBuffer: CVPixelBuffer, options: Options, time: TimeInterval) {
        var requests: [VNRequest] = []
        if options.faceTracking {
            requests.append(faceRequest)
        }
        if options.gestures {
            requests.append(handRequest)
        }
        guard !requests.isEmpty else {
            return
        }

        let handler = VNImageRequestHandler(cvPixelBuffer: pixelBuffer, orientation: .up, options: [:])
        do {
            try handler.perform(requests)
        } catch {
            failures += 1
            if failures == 1 || failures % 300 == 0 {
                EngineLog.shared.warning("vision", "analysis failed (\(failures) so far): \(error)")
            }
            return
        }

        if options.faceTracking {
            reportFace(faceRequest.results ?? [])
        }
        if options.gestures {
            let width = Double(CVPixelBufferGetWidth(pixelBuffer))
            let height = Double(CVPixelBufferGetHeight(pixelBuffer))
            let aspect = height > 0 ? width / height : 1
            let hands = (handRequest.results ?? []).compactMap { VisionAnalyzer.pose(of: $0, aspect: aspect) }
            for event in recognizer.update(hands: hands, time: time) {
                var point: PicturePoint?
                if event == .palm, let center = recognizer.lastPalmCenter, aspect > 0 {
                    // Back from the recognizer's units (picture heights, y up) to the picture.
                    point = PicturePoint(
                        x: VisionAnalyzer.rounded(min(max(center.x / aspect, 0), 1)),
                        y: VisionAnalyzer.rounded(min(max(1 - center.y, 0), 1))
                    )
                }
                onGesture?(event, point)
            }
        }
    }

    private func reportFace(_ faces: [VNFaceObservation]) {
        let largest = faces.max { left, right in
            left.boundingBox.width * left.boundingBox.height < right.boundingBox.width * right.boundingBox.height
        }
        guard let face = largest else {
            if faceVisible {
                faceVisible = false
                onFace?(nil)
            }
            return
        }
        faceVisible = true
        // Vision puts the origin at the bottom left; the protocol puts it at the top left.
        let box = face.boundingBox
        let x = min(max(Double(box.minX), 0), 1)
        let y = min(max(1 - Double(box.maxY), 0), 1)
        let width = min(max(Double(box.width), 0), 1 - x)
        let height = min(max(Double(box.height), 0), 1 - y)
        onFace?(
            PictureRect(
                x: VisionAnalyzer.rounded(x),
                y: VisionAnalyzer.rounded(y),
                width: VisionAnalyzer.rounded(width),
                height: VisionAnalyzer.rounded(height)
            )
        )
    }

    /// Converts a Vision hand into the recognizer's terms: one unit on both axes (the picture
    /// height), y growing upwards as Vision has it.
    private static func pose(of observation: VNHumanHandPoseObservation, aspect: Double) -> HandPose? {
        guard let points = try? observation.recognizedPoints(.all) else {
            return nil
        }
        var joints: [HandJoint: HandPoint] = [:]
        for (joint, name) in jointNames {
            guard let point = points[name] else {
                continue
            }
            joints[joint] = HandPoint(
                x: Double(point.location.x) * aspect,
                y: Double(point.location.y),
                confidence: Double(point.confidence)
            )
        }
        return HandPose(joints: joints)
    }

    /// Four decimals is a tenth of a pixel on a 4K picture, and keeps the messages short.
    private static func rounded(_ value: Double) -> Double {
        (value * 10_000).rounded() / 10_000
    }

    private static let jointNames: [(HandJoint, VNHumanHandPoseObservation.JointName)] = [
        (.wrist, .wrist),
        (.thumbCMC, .thumbCMC),
        (.thumbMP, .thumbMP),
        (.thumbIP, .thumbIP),
        (.thumbTip, .thumbTip),
        (.indexMCP, .indexMCP),
        (.indexPIP, .indexPIP),
        (.indexDIP, .indexDIP),
        (.indexTip, .indexTip),
        (.middleMCP, .middleMCP),
        (.middlePIP, .middlePIP),
        (.middleDIP, .middleDIP),
        (.middleTip, .middleTip),
        (.ringMCP, .ringMCP),
        (.ringPIP, .ringPIP),
        (.ringDIP, .ringDIP),
        (.ringTip, .ringTip),
        (.littleMCP, .littleMCP),
        (.littlePIP, .littlePIP),
        (.littleDIP, .littleDIP),
        (.littleTip, .littleTip),
    ]
}
