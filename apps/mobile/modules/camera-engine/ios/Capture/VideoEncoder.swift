import CoreMedia
import Foundation
import VideoToolbox

/// Compresses camera frames with the hardware encoder, tuned for delay rather than size:
/// real-time mode, no frame reordering, one frame out for every frame in.
///
/// `encode`, `configure`, `invalidate` and `requestKeyframe` must be called on one serial queue
/// (the capture video queue). Output arrives on a VideoToolbox thread.
final class VideoEncoder {
    struct Settings: Equatable {
        var codec: VideoCodec
        /// Bits per second.
        var bitrate: Int
        var fps: Int
        /// Seconds between keyframes.
        var keyframeInterval: Double
    }

    struct Output {
        /// A complete `videoFrame` message, ready to be encrypted.
        let message: Data
        let isKeyframe: Bool
        /// Present when the decoder configuration changed; send it before `message`.
        let config: VideoConfig?
        /// Size of the compressed picture alone.
        let payloadBytes: Int
    }

    /// The share of the average bitrate a one-second burst may exceed it by.
    private static let dataRateHeadroom = 1.5

    var onOutput: ((Output) -> Void)?

    private var settings: Settings
    private var session: VTCompressionSession?
    private var sessionWidth: Int32 = 0
    private var sessionHeight: Int32 = 0
    private var keyframeRequested = false
    private var failedFrames = 0

    // Shared with the output handler, which runs on another thread.
    private let outputLock = NSLock()
    private var announcedFormat: CMFormatDescription?
    private var generation = 0

    init(settings: Settings) {
        self.settings = settings
    }

    deinit {
        if let session {
            VTCompressionSessionInvalidate(session)
        }
    }

    /// Changes the settings. A new codec or frame rate restarts the encoder at the next frame;
    /// bitrate and keyframe interval are applied to the running session.
    func configure(_ newSettings: Settings) {
        let old = settings
        settings = newSettings
        guard let session else {
            return
        }
        if old.codec != newSettings.codec || old.fps != newSettings.fps {
            invalidate()
            return
        }
        if old.bitrate != newSettings.bitrate {
            applyBitrate(to: session)
        }
        if old.keyframeInterval != newSettings.keyframeInterval {
            applyKeyframeInterval(to: session)
        }
    }

    /// Makes the next frame a keyframe.
    func requestKeyframe() {
        keyframeRequested = true
    }

    /// Tears the session down. The next frame starts a new one, which begins with a fresh
    /// `videoConfig` and a keyframe.
    func invalidate() {
        if let session {
            VTCompressionSessionInvalidate(session)
        }
        session = nil
        outputLock.lock()
        announcedFormat = nil
        generation += 1
        outputLock.unlock()
    }

    func encode(_ imageBuffer: CVImageBuffer, presentationTime: CMTime, duration: CMTime) {
        let width = Int32(CVPixelBufferGetWidth(imageBuffer))
        let height = Int32(CVPixelBufferGetHeight(imageBuffer))
        if session == nil || width != sessionWidth || height != sessionHeight {
            if session != nil {
                invalidate()
            }
            session = makeSession(width: width, height: height)
            sessionWidth = width
            sessionHeight = height
        }
        guard let session else {
            return
        }

        var frameProperties: CFDictionary?
        if keyframeRequested {
            keyframeRequested = false
            frameProperties = [kVTEncodeFrameOptionKey_ForceKeyFrame as String: true] as CFDictionary
        }

        outputLock.lock()
        let frameGeneration = generation
        outputLock.unlock()
        let codec = settings.codec

        let status = VTCompressionSessionEncodeFrame(
            session,
            imageBuffer: imageBuffer,
            presentationTimeStamp: presentationTime,
            duration: duration,
            frameProperties: frameProperties,
            infoFlagsOut: nil
        ) { [weak self] status, flags, sampleBuffer in
            self?.handleOutput(
                status: status,
                flags: flags,
                sampleBuffer: sampleBuffer,
                codec: codec,
                generation: frameGeneration
            )
        }

        if status != noErr {
            failedFrames += 1
            if failedFrames == 1 || failedFrames % 120 == 0 {
                EngineLog.shared.error("encoder", "encode failed with status \(status) (\(failedFrames) so far)")
            }
            // The session can become invalid when the app returns from the background.
            invalidate()
        }
    }

    // MARK: - Session

    private func makeSession(width: Int32, height: Int32) -> VTCompressionSession? {
        let codecType: CMVideoCodecType
        switch settings.codec {
        case .h264:
            codecType = kCMVideoCodecType_H264
        case .hevc:
            codecType = kCMVideoCodecType_HEVC
        }

        var created: VTCompressionSession?
        let status = VTCompressionSessionCreate(
            allocator: kCFAllocatorDefault,
            width: width,
            height: height,
            codecType: codecType,
            encoderSpecification: nil,
            imageBufferAttributes: nil,
            compressedDataAllocator: nil,
            outputCallback: nil,
            refcon: nil,
            compressionSessionOut: &created
        )
        guard status == noErr, let session = created else {
            failedFrames += 1
            if failedFrames == 1 || failedFrames % 120 == 0 {
                EngineLog.shared.error(
                    "encoder",
                    "could not create a \(settings.codec.rawValue) session: status \(status)"
                )
            }
            return nil
        }

        set(session, kVTCompressionPropertyKey_RealTime, kCFBooleanTrue)
        set(session, kVTCompressionPropertyKey_AllowFrameReordering, kCFBooleanFalse)
        switch settings.codec {
        case .h264:
            set(session, kVTCompressionPropertyKey_ProfileLevel, kVTProfileLevel_H264_High_AutoLevel)
            set(session, kVTCompressionPropertyKey_H264EntropyMode, kVTH264EntropyMode_CABAC)
        case .hevc:
            set(session, kVTCompressionPropertyKey_ProfileLevel, kVTProfileLevel_HEVC_Main_AutoLevel)
        }
        set(session, kVTCompressionPropertyKey_ExpectedFrameRate, settings.fps as CFNumber)
        applyBitrate(to: session)
        applyKeyframeInterval(to: session)

        let prepared = VTCompressionSessionPrepareToEncodeFrames(session)
        if prepared != noErr {
            EngineLog.shared.warning("encoder", "prepare failed with status \(prepared)")
        }
        EngineLog.shared.info(
            "encoder",
            "started \(settings.codec.rawValue) \(width)x\(height) at \(settings.fps) fps, \(settings.bitrate) bit/s"
        )
        return session
    }

    private func applyBitrate(to session: VTCompressionSession) {
        set(session, kVTCompressionPropertyKey_AverageBitRate, settings.bitrate as CFNumber)
        let burstBytes = Double(settings.bitrate) * VideoEncoder.dataRateHeadroom / 8
        let limits = [burstBytes as NSNumber, 1.0 as NSNumber] as CFArray
        set(session, kVTCompressionPropertyKey_DataRateLimits, limits)
    }

    private func applyKeyframeInterval(to session: VTCompressionSession) {
        let seconds = min(max(settings.keyframeInterval, 0.1), 3600)
        let frames = max(1, Int((Double(settings.fps) * seconds).rounded()))
        set(session, kVTCompressionPropertyKey_MaxKeyFrameInterval, frames as CFNumber)
        set(session, kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration, seconds as CFNumber)
    }

    private func set(_ session: VTCompressionSession, _ key: CFString, _ value: CFTypeRef) {
        let status = VTSessionSetProperty(session, key: key, value: value)
        if status != noErr {
            EngineLog.shared.warning("encoder", "property \(key as String) was refused: status \(status)")
        }
    }

    // MARK: - Output

    private func handleOutput(
        status: OSStatus,
        flags: VTEncodeInfoFlags,
        sampleBuffer: CMSampleBuffer?,
        codec: VideoCodec,
        generation frameGeneration: Int
    ) {
        guard status == noErr, let sampleBuffer, CMSampleBufferDataIsReady(sampleBuffer) else {
            if status != noErr {
                EngineLog.shared.warning("encoder", "a frame failed to encode: status \(status)")
            }
            return
        }
        guard
            !flags.contains(.frameDropped),
            let format = CMSampleBufferGetFormatDescription(sampleBuffer),
            let blockBuffer = CMSampleBufferGetDataBuffer(sampleBuffer)
        else {
            return
        }

        let isKeyframe = VideoEncoder.isKeyframe(sampleBuffer)
        let payloadBytes = CMBlockBufferGetDataLength(blockBuffer)
        let headerBytes = 1 + VideoFrame.headerLength

        // Build the `videoFrame` message in place so the picture is copied only once.
        var message = Data(count: headerBytes + payloadBytes)
        let copyStatus = message.withUnsafeMutableBytes { raw -> OSStatus in
            guard let base = raw.baseAddress else {
                return kCMBlockBufferBadPointerParameterErr
            }
            return CMBlockBufferCopyDataBytes(
                blockBuffer,
                atOffset: 0,
                dataLength: payloadBytes,
                destination: base + headerBytes
            )
        }
        guard copyStatus == kCMBlockBufferNoErr else {
            EngineLog.shared.warning("encoder", "could not read a compressed frame: status \(copyStatus)")
            return
        }

        var header = Data(capacity: headerBytes)
        header.append(MessageKind.videoFrame)
        header.appendBigEndian(VideoEncoder.microseconds(CMSampleBufferGetPresentationTimeStamp(sampleBuffer)))
        header.append(isKeyframe ? 1 : 0)
        message.replaceSubrange(0..<headerBytes, with: header)

        outputLock.lock()
        // Frames from a session that has since been torn down would follow the wrong config.
        guard frameGeneration == generation else {
            outputLock.unlock()
            return
        }
        var config: VideoConfig?
        let formatChanged = announcedFormat.map { !CMFormatDescriptionEqual($0, otherFormatDescription: format) } ?? true
        if formatChanged {
            guard isKeyframe, let record = VideoEncoder.decoderRecord(from: format, codec: codec) else {
                outputLock.unlock()
                return
            }
            let dimensions = CMVideoFormatDescriptionGetDimensions(format)
            // The capture connection already rotated and mirrored the picture, so the host has
            // nothing left to do: rotation is always 0 and the mirror flag is never set.
            config = VideoConfig(
                codec: codec,
                width: UInt16(clamping: dimensions.width),
                height: UInt16(clamping: dimensions.height),
                rotation: 0,
                mirrored: false,
                record: record
            )
            announcedFormat = format
        }
        outputLock.unlock()

        onOutput?(
            Output(message: message, isKeyframe: isKeyframe, config: config, payloadBytes: payloadBytes)
        )
    }

    private static func isKeyframe(_ sampleBuffer: CMSampleBuffer) -> Bool {
        guard
            let attachments = CMSampleBufferGetSampleAttachmentsArray(sampleBuffer, createIfNecessary: false)
                as? [[CFString: Any]],
            let first = attachments.first
        else {
            return true
        }
        let notSync = first[kCMSampleAttachmentKey_NotSync] as? Bool ?? false
        return !notSync
    }

    /// The `avcC` or `hvcC` box the encoder attached to its output format.
    private static func decoderRecord(from format: CMFormatDescription, codec: VideoCodec) -> Data? {
        guard
            let atoms = CMFormatDescriptionGetExtension(
                format,
                extensionKey: kCMFormatDescriptionExtension_SampleDescriptionExtensionAtoms
            ) as? [String: Any]
        else {
            return nil
        }
        switch codec {
        case .h264:
            return atoms["avcC"] as? Data
        case .hevc:
            return atoms["hvcC"] as? Data
        }
    }

    private static func microseconds(_ time: CMTime) -> UInt64 {
        guard time.isValid, !time.isIndefinite else {
            return 0
        }
        let converted = CMTimeConvertScale(time, timescale: 1_000_000, method: .default)
        return UInt64(max(0, converted.value))
    }
}
