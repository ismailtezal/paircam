# Prior art

Open-source projects studied before starting, what each one teaches, and whether its code may be
reused. Kadraj is MIT, so only MIT, BSD and Apache code can be copied (with notices kept).
GPL projects are for reading only.

## Phone side

| Project | Licence | What it teaches |
| --- | --- | --- |
| [Moblin](https://github.com/eerimoq/moblin) | MIT | A complete iOS capture and encode pipeline in Swift: 4K60, HEVC, lens selection, stabilization. Its documented Mobcam protocol is the model for our framing and flow control. |
| [HaishinKit](https://github.com/HaishinKit/HaishinKit.swift) | BSD-3 | Camera and microphone streaming library for Apple platforms. |
| [scrcpy](https://github.com/Genymobile/scrcpy) | Apache-2.0 | The reference for low-delay streaming from Android: no buffering, decode and show immediately. |

## Desktop side

| Project | Licence | What it teaches |
| --- | --- | --- |
| [VCamSample](https://github.com/smourier/VCamSample) | MIT | Minimal Windows 11 Media Foundation virtual camera. |
| [Windows-Camera samples](https://github.com/microsoft/Windows-Camera/tree/master/Samples/VirtualCamera) | MIT | Microsoft's official virtual camera sample, with installer and tests. |
| [neon](https://github.com/newdee/neon) | MIT | A C++ engine feeding a Media Foundation virtual camera over a named pipe, with a Tauri 2 control app. |
| [softcam](https://github.com/tshino/softcam) | MIT | The older DirectShow virtual camera. Not used: apps on the newer Windows camera stack do not see it. |

## Read only (GPL)

| Project | Licence | What it teaches |
| --- | --- | --- |
| [Mobcam](https://github.com/eerimoq/mobcam) | GPL-2.0 | Receiving an iPhone stream on Windows over Apple's USB tunnel; FFmpeg hardware decoding. |
| [DroidCam OBS plugin](https://github.com/dev47apps/droidcam-obs-plugin) | GPL-2.0 | Wi-Fi and USB receiving, device discovery, decoding. |
| [obs-ios-camera-source](https://github.com/wtsnz/obs-ios-camera-source) | GPL-2.0 | iPhone to PC over USB. |
| [Android Webcam Project](https://github.com/soubhagyajit/Android-Webcam-Project) | GPL-3.0 | A Kotlin phone app with a desktop client. |

## Known pitfalls from their issue trackers

- A Media Foundation virtual camera's files must be readable by Windows' camera services; under
  a user profile folder, starting the camera fails with "Access denied".
- The camera source can only be registered machine-wide, which needs administrator rights once.
- One sample showed video in Microsoft Teams' own preview but not to the other participant, so
  Teams is tested early with a second account.
- Whether two apps can open the camera at once depends on how each app opens it.
- 60 fps output only happens if frames are delivered at that pace; there is no setting for it.
