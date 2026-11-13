// Hand gesture classification and recognition, fed with synthetic hands. Each gesture is checked
// together with the near misses that must not trigger it.

import XCTest

@testable import PairCamGestures

private enum FingerState {
    case extended
    case curled
}

private enum ThumbState {
    /// Spread away from the palm, up and out, as in an open hand.
    case open
    /// Straight out to the side, square to the index finger: the L of `frame`.
    case side
    /// Folded across the palm.
    case tucked
}

private let analysisRate = 15.0

/// Builds a right hand facing the camera. The layout is in palm lengths with the wrist at the
/// origin and the fingers pointing up; it is then rotated about the wrist, scaled to `size`
/// picture heights and moved to `wrist`.
private func makeHand(
    wrist: (x: Double, y: Double) = (0.6, 0.3),
    size: Double = 0.15,
    rotation: Double = 0,
    thumb: ThumbState = .open,
    index: FingerState = .extended,
    middle: FingerState = .extended,
    ring: FingerState = .extended,
    little: FingerState = .extended,
    confidence: Double = 0.9
) -> HandPose {
    var local: [HandJoint: (Double, Double)] = [.wrist: (0, 0)]

    func finger(_ joints: [HandJoint], knuckle: (Double, Double), state: FingerState) {
        let offsets: [Double] = state == .extended ? [0, 0.4, 0.65, 0.85] : [0, 0.3, 0.15, -0.05]
        for (joint, offset) in zip(joints, offsets) {
            local[joint] = (knuckle.0, knuckle.1 + offset)
        }
    }
    finger(Finger.index.joints, knuckle: (-0.3, 1.0), state: index)
    finger(Finger.middle.joints, knuckle: (-0.1, 1.0), state: middle)
    finger(Finger.ring.joints, knuckle: (0.1, 0.95), state: ring)
    finger(Finger.little.joints, knuckle: (0.3, 0.85), state: little)

    let base = (-0.25, 0.25)
    local[.thumbCMC] = base
    switch thumb {
    case .open, .side:
        let direction = thumb == .open ? (-0.7071, 0.7071) : (-1.0, 0.0)
        local[.thumbMP] = (base.0 + direction.0 * 0.35, base.1 + direction.1 * 0.35)
        local[.thumbIP] = (base.0 + direction.0 * 0.65, base.1 + direction.1 * 0.65)
        local[.thumbTip] = (base.0 + direction.0 * 0.95, base.1 + direction.1 * 0.95)
    case .tucked:
        local[.thumbMP] = (base.0 + 0.05, base.1 + 0.25)
        local[.thumbIP] = (base.0 + 0.1, base.1 + 0.4)
        local[.thumbTip] = (base.0 + 0.15, base.1 + 0.5)
    }

    let radians = rotation * Double.pi / 180
    var joints: [HandJoint: HandPoint] = [:]
    for (joint, point) in local {
        let x = point.0 * cos(radians) - point.1 * sin(radians)
        let y = point.0 * sin(radians) + point.1 * cos(radians)
        joints[joint] = HandPoint(x: wrist.x + x * size, y: wrist.y + y * size, confidence: confidence)
    }
    return HandPose(joints: joints)
}

private let openPalm = makeHand()
private let lFrame = makeHand(thumb: .side, middle: .curled, ring: .curled, little: .curled)
private let fist = makeHand(thumb: .tucked, index: .curled, middle: .curled, ring: .curled, little: .curled)

/// Runs the recognizer over `frames` analyses at the analysis rate, starting at `start` seconds.
private func simulate(
    _ recognizer: inout HandGestureRecognizer,
    from start: Double = 0,
    frames: Int,
    hands: (Int) -> [HandPose]
) -> [HandGestureRecognizer.Event] {
    var events: [HandGestureRecognizer.Event] = []
    for frame in 0..<frames {
        let time = start + Double(frame) / analysisRate
        events.append(contentsOf: recognizer.update(hands: hands(frame), time: time))
    }
    return events
}

private func oneOffs(_ events: [HandGestureRecognizer.Event]) -> [HandGestureRecognizer.Event] {
    events.filter { $0 == .palm || $0 == .frame }
}

final class HandShapeTests: XCTestCase {
    func testOpenPalmIsRecognised() {
        XCTAssertTrue(HandGeometry.isReliable(openPalm))
        XCTAssertEqual(HandGeometry.shape(of: openPalm), .openPalm)
        for finger in Finger.allCases {
            XCTAssertTrue(HandGeometry.isExtended(finger, in: openPalm), "\(finger)")
        }
    }

    func testOpenPalmSlightlyTiltedStillCounts() {
        XCTAssertEqual(HandGeometry.shape(of: makeHand(rotation: 30)), .openPalm)
        XCTAssertEqual(HandGeometry.shape(of: makeHand(rotation: -30)), .openPalm)
    }

    func testSidewaysOrDownwardOpenHandIsNotAPalm() {
        XCTAssertEqual(HandGeometry.shape(of: makeHand(rotation: 90)), .other)
        XCTAssertEqual(HandGeometry.shape(of: makeHand(rotation: -90)), .other)
        XCTAssertEqual(HandGeometry.shape(of: makeHand(rotation: 180)), .other)
    }

    func testPalmWithAFingerDownIsNotAPalm() {
        XCTAssertEqual(HandGeometry.shape(of: makeHand(ring: .curled)), .other)
        XCTAssertEqual(HandGeometry.shape(of: makeHand(thumb: .tucked)), .other)
    }

    func testLShapeIsAFrame() {
        XCTAssertEqual(HandGeometry.shape(of: lFrame), .frame)
        XCTAssertTrue(HandGeometry.isCurled(.middle, in: lFrame))
        XCTAssertFalse(HandGeometry.isExtended(.middle, in: lFrame))
    }

    func testFrameNearMisses() {
        // Pointing with the thumb folded away.
        let pointing = makeHand(thumb: .tucked, middle: .curled, ring: .curled, little: .curled)
        XCTAssertEqual(HandGeometry.shape(of: pointing), .other)
        // Thumb and index too close to parallel.
        let narrow = makeHand(thumb: .open, middle: .curled, ring: .curled, little: .curled)
        XCTAssertEqual(HandGeometry.shape(of: narrow), .other)
        // Middle finger up as well.
        let twoFingers = makeHand(thumb: .side, ring: .curled, little: .curled)
        XCTAssertEqual(HandGeometry.shape(of: twoFingers), .other)
    }

    func testFistIsNothing() {
        XCTAssertEqual(HandGeometry.shape(of: fist), .other)
        XCTAssertFalse(HandGeometry.isOpen(fist))
    }

    func testTinyOrUncertainHandsAreIgnored() {
        let tiny = makeHand(size: 0.03)
        XCTAssertFalse(HandGeometry.isReliable(tiny))
        XCTAssertEqual(HandGeometry.shape(of: tiny), .other)

        let uncertain = makeHand(confidence: 0.3)
        XCTAssertFalse(HandGeometry.isReliable(uncertain))
        XCTAssertEqual(HandGeometry.shape(of: uncertain), .other)

        var missingJoint = openPalm
        missingJoint.joints[.littleTip] = nil
        XCTAssertFalse(HandGeometry.isReliable(missingJoint))
    }
}

final class OneOffGestureTests: XCTestCase {
    func testPalmFiresOnceAfterBeingHeldForASecond() {
        var recognizer = HandGestureRecognizer()
        let early = simulate(&recognizer, frames: 14) { _ in [openPalm] }
        XCTAssertEqual(oneOffs(early), [])

        let rest = simulate(&recognizer, from: 14 / analysisRate, frames: 40) { _ in [openPalm] }
        XCTAssertEqual(oneOffs(rest), [.palm])
    }

    func testPalmRemembersWhereTheHandWas() {
        var recognizer = HandGestureRecognizer()
        XCTAssertNil(recognizer.lastPalmCenter)
        let events = simulate(&recognizer, frames: 20) { _ in [openPalm] }
        XCTAssertEqual(oneOffs(events), [.palm])
        // The wrist is at (0.6, 0.3), and the wrist and knuckles average 0.76 palm lengths
        // straight above it.
        let center = try? XCTUnwrap(recognizer.lastPalmCenter)
        XCTAssertEqual(center?.x ?? 0, 0.6, accuracy: 1e-9)
        XCTAssertEqual(center?.y ?? 0, 0.3 + 0.76 * 0.15, accuracy: 1e-9)

        _ = recognizer.reset()
        XCTAssertNil(recognizer.lastPalmCenter)
    }

    func testFrameFiresAfterBeingHeld() {
        var recognizer = HandGestureRecognizer()
        let events = simulate(&recognizer, frames: 20) { _ in [lFrame] }
        XCTAssertEqual(oneOffs(events), [.frame])
    }

    func testABriefHoldDoesNothing() {
        var recognizer = HandGestureRecognizer()
        let events = simulate(&recognizer, frames: 30) { frame in frame < 12 ? [openPalm] : [] }
        XCTAssertEqual(events, [])
    }

    func testAShortDropoutDoesNotBreakTheHold() {
        var recognizer = HandGestureRecognizer()
        let events = simulate(&recognizer, frames: 18) { frame in
            frame == 7 || frame == 8 ? [] : [openPalm]
        }
        XCTAssertEqual(oneOffs(events), [.palm])
    }

    func testALongDropoutRestartsTheHold() {
        var recognizer = HandGestureRecognizer()
        let events = simulate(&recognizer, frames: 18) { frame in
            (5...10).contains(frame) ? [] : [openPalm]
        }
        XCTAssertEqual(events, [])
    }

    func testCoolDownAndReleaseAfterFiring() {
        var recognizer = HandGestureRecognizer()
        // Fires at 1.0 s; the cool-down runs to 3.0 s.
        let first = simulate(&recognizer, frames: 18) { _ in [openPalm] }
        XCTAssertEqual(oneOffs(first), [.palm])
        // Let go, then show the palm again during the cool-down: nothing.
        let during = simulate(&recognizer, from: 18 / analysisRate, frames: 25) { frame in
            frame < 6 ? [] : [openPalm]
        }
        XCTAssertEqual(during, [])
        // Keep holding past the end of the cool-down: it fires again.
        let after = simulate(&recognizer, from: 43 / analysisRate, frames: 10) { _ in [openPalm] }
        XCTAssertEqual(oneOffs(after), [.palm])
    }

    func testHoldingOnDoesNotRepeat() {
        var recognizer = HandGestureRecognizer()
        let events = simulate(&recognizer, frames: Int(analysisRate * 6)) { _ in [openPalm] }
        XCTAssertEqual(oneOffs(events), [.palm])
    }

    func testFistsTinyHandsAndUncertainHandsNeverFire() {
        var recognizer = HandGestureRecognizer()
        let fists = simulate(&recognizer, frames: 45) { _ in [fist] }
        XCTAssertEqual(fists, [])
        let tiny = simulate(&recognizer, from: 3, frames: 45) { _ in [makeHand(size: 0.03)] }
        XCTAssertEqual(tiny, [])
        let uncertain = simulate(&recognizer, from: 6, frames: 45) { _ in [makeHand(confidence: 0.3)] }
        XCTAssertEqual(uncertain, [])
    }

    func testTwoDifferentGesturesAtOnceAreIgnored() {
        var recognizer = HandGestureRecognizer()
        let other = makeHand(
            wrist: (1.2, 0.3),
            thumb: .side,
            middle: .curled,
            ring: .curled,
            little: .curled
        )
        let events = simulate(&recognizer, frames: 30) { _ in [openPalm, other] }
        XCTAssertEqual(events, [])
    }
}

final class SpreadGestureTests: XCTestCase {
    private let left = makeHand(wrist: (0.4, 0.3))

    private func right(at x: Double) -> HandPose {
        makeHand(wrist: (x, 0.3))
    }

    func testHandsMovingApartZoomIn() {
        var recognizer = HandGestureRecognizer()
        // Rest, move apart for a second, then hold still.
        let events = simulate(&recognizer, frames: 60) { frame in
            let moving = min(max(frame - 8, 0), 15)
            return [self.left, self.right(at: 1.1 + Double(moving) * 0.03)]
        }
        guard let began = events.firstIndex(of: .spreadBegan) else {
            return XCTFail("no spreadBegan in \(events)")
        }
        guard let ended = events.firstIndex(of: .spreadEnded) else {
            return XCTFail("no spreadEnded in \(events)")
        }
        XCTAssertLessThan(began, ended)
        let scales = events.compactMap { event -> Double? in
            if case .spreadChanged(let scale) = event {
                return scale
            }
            return nil
        }
        XCTAssertFalse(scales.isEmpty)
        XCTAssertGreaterThan(scales.last ?? 0, 1.2)
        XCTAssertEqual(oneOffs(events), [])
    }

    func testHandsMovingTogetherZoomOut() {
        var recognizer = HandGestureRecognizer()
        let events = simulate(&recognizer, frames: 30) { frame in
            let moving = min(max(frame - 8, 0), 15)
            return [self.left, self.right(at: 1.4 - Double(moving) * 0.03)]
        }
        XCTAssertTrue(events.contains(.spreadBegan))
        let scales = events.compactMap { event -> Double? in
            if case .spreadChanged(let scale) = event {
                return scale
            }
            return nil
        }
        XCTAssertLessThan(scales.last ?? 1, 0.85)
    }

    func testStillHandsDoNothing() {
        var recognizer = HandGestureRecognizer()
        let events = simulate(&recognizer, frames: 45) { _ in [self.left, self.right(at: 1.1)] }
        XCTAssertEqual(events, [])
    }

    func testSpreadEndsWhenAHandLeaves() {
        var recognizer = HandGestureRecognizer()
        let events = simulate(&recognizer, frames: 40) { frame in
            if frame >= 20 {
                return [self.left]
            }
            return [self.left, self.right(at: 1.1 + Double(frame) * 0.03)]
        }
        XCTAssertTrue(events.contains(.spreadBegan))
        XCTAssertEqual(events.last, .spreadEnded)
        XCTAssertEqual(events.filter { $0 == .spreadEnded }.count, 1)
        // The hand left behind is an open palm, but the cool-down after a spread holds it off.
        XCTAssertEqual(oneOffs(events), [])
    }

    func testResetEndsASpreadInProgress() {
        var recognizer = HandGestureRecognizer()
        _ = simulate(&recognizer, frames: 15) { frame in
            [self.left, self.right(at: 1.1 + Double(frame) * 0.03)]
        }
        XCTAssertTrue(recognizer.isSpreading)
        let ended = recognizer.reset()
        XCTAssertEqual(ended, [.spreadEnded])
        XCTAssertFalse(recognizer.isSpreading)
        let again = recognizer.reset()
        XCTAssertEqual(again, [])
    }
}

final class CrossGeometryTests: XCTestCase {
    func testCrossingSegmentsAreDetected() {
        let a = HandPoint(x: 0, y: 0)
        let b = HandPoint(x: 1, y: 1)
        let c = HandPoint(x: 0, y: 1)
        let d = HandPoint(x: 1, y: 0)
        XCTAssertTrue(HandGeometry.segmentsCross(a, b, c, d))
    }

    func testSegmentsThatDoNotMeetAreNotACross() {
        let a = HandPoint(x: 0, y: 0)
        let b = HandPoint(x: 1, y: 0)
        // Parallel.
        XCTAssertFalse(HandGeometry.segmentsCross(a, b, HandPoint(x: 0, y: 1), HandPoint(x: 1, y: 1)))
        // Pointing at each other without touching.
        XCTAssertFalse(HandGeometry.segmentsCross(a, b, HandPoint(x: 2, y: -1), HandPoint(x: 2, y: 1)))
    }

    func testOneHandIsNeverACross() {
        XCTAssertFalse(HandGeometry.isCross([]))
        XCTAssertFalse(HandGeometry.isCross([HandPose(joints: [:])]))
    }
}
