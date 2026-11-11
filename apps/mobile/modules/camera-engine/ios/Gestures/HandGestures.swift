// Hand gesture recognition as pure functions over joint positions. Nothing here knows about
// Vision or the camera: `VisionAnalyzer` turns Vision's hand observations into `HandPose` values
// and feeds them to `HandGestureRecognizer`, which says when a gesture fires. Keeping this file
// to Foundation lets the Swift test package check every gesture and its near misses with
// synthetic hands.
//
// Positions use one unit on both axes (the analyzer uses the picture height), with y growing
// upwards, the way Vision reports them.

import Foundation

/// Every threshold that decides whether a gesture fires. They are guesses until tuned on a real
/// phone, so they all live here.
enum GestureTuning {
    // MARK: Analysis

    /// The shortest time between two analysed frames: at most 15 analyses a second, shared by
    /// face detection and hand pose detection.
    static let analysisInterval: TimeInterval = 1.0 / 15.0

    // MARK: Which hands count

    /// Every joint a gesture looks at must be at least this sure (Vision reports 0 to 1).
    static let minJointConfidence = 0.5
    /// Hands whose palm (wrist to middle knuckle) is shorter than this share of the picture
    /// height are ignored: they are far away or in the background.
    static let minPalmSize = 0.05

    // MARK: Finger shapes

    /// A finger is straight when the distance from knuckle to tip is at least this share of the
    /// length along its joints (1 is perfectly straight).
    static let straightFinger = 0.85
    /// A straight finger must also reach this much further from the wrist than its middle
    /// joint does, so a finger pointing at the camera does not count.
    static let extendedReach = 1.1
    /// A finger is curled when its tip is no further from the wrist than this share of its
    /// middle joint's distance.
    static let curledReach = 1.05
    /// The thumb is straight when its last two segments are at least this straight.
    static let straightThumb = 0.8
    /// An extended thumb's tip is at least this many palm lengths from the index knuckle.
    static let thumbReach = 0.5
    /// An open palm is "held up": the line from wrist to middle knuckle is within this many
    /// degrees of straight up.
    static let palmUprightDegrees = 50.0
    /// In the L of `frame`, the thumb and index finger meet at an angle in this range, in
    /// degrees.
    static let frameAngleDegrees = 60.0...120.0

    // MARK: One-off gestures (palm, frame)

    /// How long a pose must be held before it fires.
    static let holdDuration: TimeInterval = 1.0
    /// A pose may drop out of detection for this long without breaking the hold.
    static let holdGrace: TimeInterval = 0.25
    /// After a gesture fires (or a spread ends), no one-off gesture fires for this long.
    static let cooldown: TimeInterval = 2.0

    // MARK: Spread

    /// The distance between the hands, in palm lengths, must change by this share before a
    /// spread begins.
    static let spreadStartChange = 0.15
    /// Changes smaller than this share count as holding still.
    static let spreadStillChange = 0.04
    /// A spread ends when the hands hold still this long.
    static let spreadStillDuration: TimeInterval = 0.6
    /// A spread survives a hand dropping out of detection for this long.
    static let spreadLostGrace: TimeInterval = 0.3
    /// Weight of the newest measurement when smoothing the distance between the hands (0 to 1).
    static let spreadSmoothing = 0.5
    /// Zoom follows (distance now / distance at the start) raised to this power.
    static let spreadZoomExponent = 1.5

    /// Two index fingers count as an X when the angle between them is in this range.
    static let crossAngleDegrees = 40.0...140.0

    // MARK: Zoom targets

    /// `frame` multiplies the zoom by this factor and `cross` divides it, never below 1×.
    static let zoomStep = 2.0
}

enum HandJoint: CaseIterable, Hashable {
    case wrist
    case thumbCMC
    case thumbMP
    case thumbIP
    case thumbTip
    case indexMCP
    case indexPIP
    case indexDIP
    case indexTip
    case middleMCP
    case middlePIP
    case middleDIP
    case middleTip
    case ringMCP
    case ringPIP
    case ringDIP
    case ringTip
    case littleMCP
    case littlePIP
    case littleDIP
    case littleTip
}

enum Finger: CaseIterable {
    case thumb
    case index
    case middle
    case ring
    case little

    /// The finger's joints from the base to the tip.
    var joints: [HandJoint] {
        switch self {
        case .thumb:
            return [.thumbCMC, .thumbMP, .thumbIP, .thumbTip]
        case .index:
            return [.indexMCP, .indexPIP, .indexDIP, .indexTip]
        case .middle:
            return [.middleMCP, .middlePIP, .middleDIP, .middleTip]
        case .ring:
            return [.ringMCP, .ringPIP, .ringDIP, .ringTip]
        case .little:
            return [.littleMCP, .littlePIP, .littleDIP, .littleTip]
        }
    }
}

struct HandPoint: Equatable {
    var x: Double
    var y: Double
    /// From 0 to 1.
    var confidence: Double

    init(x: Double, y: Double, confidence: Double = 1) {
        self.x = x
        self.y = y
        self.confidence = confidence
    }

    func distance(to other: HandPoint) -> Double {
        hypot(other.x - x, other.y - y)
    }
}

/// One hand: the position of each joint the detector found.
struct HandPose: Equatable {
    var joints: [HandJoint: HandPoint]

    init(joints: [HandJoint: HandPoint]) {
        self.joints = joints
    }

    subscript(_ joint: HandJoint) -> HandPoint? {
        joints[joint]
    }
}

/// What one hand is doing.
enum HandShape: Hashable {
    /// All five fingers extended, held up.
    case openPalm
    /// Thumb and index finger extended at about a right angle, the other three curled.
    case frame
    /// Two hands with their index fingers crossed in an X. A two-hand shape: see `isCross`.
    case cross
    case other
}

// MARK: - Classification

enum HandGeometry {
    /// Wrist to middle knuckle: the hand's size in the picture.
    static func palmSize(_ hand: HandPose) -> Double? {
        guard let wrist = hand[.wrist], let middle = hand[.middleMCP] else {
            return nil
        }
        return wrist.distance(to: middle)
    }

    /// The hand is big enough and every joint is confidently placed.
    static func isReliable(_ hand: HandPose) -> Bool {
        for joint in HandJoint.allCases {
            guard let point = hand[joint], point.confidence >= GestureTuning.minJointConfidence else {
                return false
            }
        }
        guard let size = palmSize(hand) else {
            return false
        }
        return size >= GestureTuning.minPalmSize
    }

    /// The middle of the palm: the average of the wrist and the four knuckles.
    static func palmCenter(_ hand: HandPose) -> HandPoint? {
        let joints: [HandJoint] = [.wrist, .indexMCP, .middleMCP, .ringMCP, .littleMCP]
        var x = 0.0
        var y = 0.0
        for joint in joints {
            guard let point = hand[joint] else {
                return nil
            }
            x += point.x
            y += point.y
        }
        return HandPoint(x: x / Double(joints.count), y: y / Double(joints.count))
    }

    /// Distance from the first to the last point divided by the length along all of them.
    static func straightness(_ points: [HandPoint]) -> Double {
        guard let first = points.first, let last = points.last, points.count > 1 else {
            return 0
        }
        var path = 0.0
        for index in 1..<points.count {
            path += points[index - 1].distance(to: points[index])
        }
        guard path > 0 else {
            return 0
        }
        return first.distance(to: last) / path
    }

    static func isExtended(_ finger: Finger, in hand: HandPose) -> Bool {
        let points = finger.joints.compactMap { hand[$0] }
        guard points.count == 4, let wrist = hand[.wrist] else {
            return false
        }
        if finger == .thumb {
            guard let indexKnuckle = hand[.indexMCP], let size = palmSize(hand), size > 0 else {
                return false
            }
            let straight = straightness(Array(points[1...])) >= GestureTuning.straightThumb
            let reach = points[3].distance(to: indexKnuckle) / size
            return straight && reach >= GestureTuning.thumbReach
        }
        let straight = straightness(points) >= GestureTuning.straightFinger
        let tipReach = wrist.distance(to: points[3])
        let middleReach = wrist.distance(to: points[1])
        return straight && tipReach >= middleReach * GestureTuning.extendedReach
    }

    static func isCurled(_ finger: Finger, in hand: HandPose) -> Bool {
        guard finger != .thumb else {
            return !isExtended(finger, in: hand)
        }
        let points = finger.joints.compactMap { hand[$0] }
        guard points.count == 4, let wrist = hand[.wrist] else {
            return false
        }
        let tipReach = wrist.distance(to: points[3])
        let middleReach = wrist.distance(to: points[1])
        return tipReach <= middleReach * GestureTuning.curledReach
    }

    /// The four fingers (not the thumb) are extended. Both hands of a spread must be open.
    static func isOpen(_ hand: HandPose) -> Bool {
        [Finger.index, .middle, .ring, .little].allSatisfy { isExtended($0, in: hand) }
    }

    /// Angle in degrees between the direction from `a` to `b` and the direction from `c` to `d`,
    /// from 0 to 180.
    static func angle(_ a: HandPoint, _ b: HandPoint, between c: HandPoint, _ d: HandPoint) -> Double {
        let first = (x: b.x - a.x, y: b.y - a.y)
        let second = (x: d.x - c.x, y: d.y - c.y)
        let lengths = hypot(first.x, first.y) * hypot(second.x, second.y)
        guard lengths > 0 else {
            return 0
        }
        let cosine = (first.x * second.x + first.y * second.y) / lengths
        return acos(min(max(cosine, -1), 1)) * 180 / Double.pi
    }

    /// Whether the segment from `a` to `b` crosses the segment from `c` to `d`.
    static func segmentsCross(_ a: HandPoint, _ b: HandPoint, _ c: HandPoint, _ d: HandPoint) -> Bool {
        func side(_ p: HandPoint, _ q: HandPoint, _ r: HandPoint) -> Double {
            (q.x - p.x) * (r.y - p.y) - (q.y - p.y) * (r.x - p.x)
        }
        let d1 = side(c, d, a)
        let d2 = side(c, d, b)
        let d3 = side(a, b, c)
        let d4 = side(a, b, d)
        return d1 * d2 < 0 && d3 * d4 < 0
    }

    /// Whether two hands hold their index fingers crossed in an X.
    static func isCross(_ hands: [HandPose]) -> Bool {
        guard hands.count == 2 else {
            return false
        }
        let fingers = hands.compactMap { hand -> (base: HandPoint, tip: HandPoint)? in
            guard isExtended(.index, in: hand), let base = hand[.indexMCP], let tip = hand[.indexTip] else {
                return nil
            }
            return (base: base, tip: tip)
        }
        guard fingers.count == 2 else {
            return false
        }
        let first = fingers[0]
        let second = fingers[1]
        let between = angle(first.base, first.tip, between: second.base, second.tip)
        return GestureTuning.crossAngleDegrees.contains(between)
            && segmentsCross(first.base, first.tip, second.base, second.tip)
    }

    static func shape(of hand: HandPose) -> HandShape {
        guard isReliable(hand), let wrist = hand[.wrist], let middle = hand[.middleMCP] else {
            return .other
        }
        let thumb = isExtended(.thumb, in: hand)
        let index = isExtended(.index, in: hand)

        if thumb, isOpen(hand) {
            let up = HandPoint(x: wrist.x, y: wrist.y + 1)
            let tilt = angle(wrist, middle, between: wrist, up)
            return tilt <= GestureTuning.palmUprightDegrees ? .openPalm : .other
        }

        let othersCurled = [Finger.middle, .ring, .little].allSatisfy { isCurled($0, in: hand) }
        if thumb, index, othersCurled,
            let thumbBase = hand[.thumbMP],
            let thumbTip = hand[.thumbTip],
            let indexBase = hand[.indexMCP],
            let indexTip = hand[.indexTip]
        {
            let between = angle(thumbBase, thumbTip, between: indexBase, indexTip)
            if GestureTuning.frameAngleDegrees.contains(between) {
                return .frame
            }
        }
        return .other
    }
}

// MARK: - Recognition over time

/// Turns hand poses from successive frames into gestures: holds, cool-downs and the spread.
/// Feed it every analysed frame, including frames with no hands.
struct HandGestureRecognizer {
    enum Event: Equatable {
        case palm
        case frame
        case cross
        case spreadBegan
        /// The distance between the hands now, divided by the distance when the spread began.
        case spreadChanged(scale: Double)
        case spreadEnded
    }

    private struct Hold {
        var shape: HandShape
        var since: TimeInterval
        var lastSeen: TimeInterval
    }

    private struct Spread {
        /// Smoothed distance between the hands, in palm lengths.
        var distance: Double
        /// The distance the next spread is measured from.
        var anchor: Double
        var active = false
        var lastMovedDistance: Double
        var lastMovedAt: TimeInterval
        var lastSeen: TimeInterval
    }

    private var hold: Hold?
    /// A gesture that fired and must be let go of before it can fire again.
    private var released: (shape: HandShape, lastSeen: TimeInterval)?
    private var cooldownUntil: TimeInterval = -Double.infinity
    private var spread: Spread?

    var isSpreading: Bool {
        spread?.active ?? false
    }

    /// Forgets everything. Returns `spreadEnded` if a spread was under way.
    mutating func reset() -> [Event] {
        let wasSpreading = isSpreading
        hold = nil
        released = nil
        spread = nil
        cooldownUntil = -Double.infinity
        return wasSpreading ? [.spreadEnded] : []
    }

    mutating func update(hands: [HandPose], time: TimeInterval) -> [Event] {
        let usable = hands.filter { HandGeometry.isReliable($0) }
        var events = updateSpread(hands: usable, time: time)
        if spread != nil {
            // Two open hands are in view (or were a moment ago): no one-off gesture.
            hold = nil
            return events
        }
        if let event = updateOneOff(hands: usable, time: time) {
            events.append(event)
        }
        return events
    }

    private mutating func updateSpread(hands: [HandPose], time: TimeInterval) -> [Event] {
        let open = hands.filter { HandGeometry.isOpen($0) }
        guard
            open.count == 2,
            let first = HandGeometry.palmCenter(open[0]),
            let second = HandGeometry.palmCenter(open[1]),
            let firstSize = HandGeometry.palmSize(open[0]),
            let secondSize = HandGeometry.palmSize(open[1])
        else {
            guard let current = spread else {
                return []
            }
            if current.active, time - current.lastSeen <= GestureTuning.spreadLostGrace {
                return []
            }
            spread = nil
            if current.active {
                cooldownUntil = time + GestureTuning.cooldown
                return [.spreadEnded]
            }
            return []
        }

        // In palm lengths, so the measure does not change when the camera zooms or the person
        // steps back.
        let measured = first.distance(to: second) / ((firstSize + secondSize) / 2)
        guard var current = spread else {
            spread = Spread(
                distance: measured,
                anchor: measured,
                lastMovedDistance: measured,
                lastMovedAt: time,
                lastSeen: time
            )
            return []
        }

        current.distance += (measured - current.distance) * GestureTuning.spreadSmoothing
        current.lastSeen = time
        let distance = current.distance
        var events: [Event] = []

        if abs(distance - current.lastMovedDistance) >= current.lastMovedDistance * GestureTuning.spreadStillChange {
            current.lastMovedDistance = distance
            current.lastMovedAt = time
        }
        let still = time - current.lastMovedAt >= GestureTuning.spreadStillDuration

        if current.active {
            if still {
                current.active = false
                current.anchor = distance
                cooldownUntil = time + GestureTuning.cooldown
                events.append(.spreadEnded)
            } else {
                events.append(.spreadChanged(scale: distance / current.anchor))
            }
        } else if abs(distance - current.anchor) >= current.anchor * GestureTuning.spreadStartChange {
            current.active = true
            events.append(.spreadBegan)
            events.append(.spreadChanged(scale: distance / current.anchor))
        } else if still {
            // Hands resting in view drift slowly; measure the next spread from where they are.
            current.anchor = distance
        }
        spread = current
        return events
    }

    private mutating func updateOneOff(hands: [HandPose], time: TimeInterval) -> Event? {
        let shapes = Set(hands.map { HandGeometry.shape(of: $0) }).subtracting([.other])
        // Two different gestures at once is ambiguous: neither counts.
        let candidate: HandShape? = HandGeometry.isCross(hands) ? .cross : (shapes.count == 1 ? shapes.first : nil)

        if let pending = released {
            if candidate == pending.shape {
                released = (shape: pending.shape, lastSeen: time)
                return nil
            }
            if time - pending.lastSeen <= GestureTuning.holdGrace {
                return nil
            }
            released = nil
        }

        guard let shape = candidate else {
            if let current = hold, time - current.lastSeen > GestureTuning.holdGrace {
                hold = nil
            }
            return nil
        }
        guard var current = hold, current.shape == shape else {
            hold = Hold(shape: shape, since: time, lastSeen: time)
            return nil
        }
        current.lastSeen = time
        hold = current

        guard time - current.since >= GestureTuning.holdDuration, time >= cooldownUntil else {
            return nil
        }
        hold = nil
        released = (shape: shape, lastSeen: time)
        cooldownUntil = time + GestureTuning.cooldown
        switch shape {
        case .openPalm:
            return .palm
        case .frame:
            return .frame
        case .cross:
            return .cross
        case .other:
            return nil
        }
    }
}
