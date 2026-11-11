Pod::Spec.new do |s|
  s.name           = 'PairCamCameraEngine'
  s.version        = '0.1.0'
  s.summary        = 'The PairCam camera engine: capture, encode, pair and stream.'
  s.description    = 'Native Swift module that owns the camera, the hardware encoder and the PairCam protocol.'
  s.license        = 'MIT'
  s.author         = 'PairCam contributors'
  s.homepage       = 'https://github.com/ismailtezal/paircam'
  s.platforms      = {
    :ios => '16.4'
  }
  s.swift_version  = '5.9'
  s.source         = { git: 'https://github.com/ismailtezal/paircam.git' }
  s.static_framework = true

  s.dependency 'ExpoModulesCore'

  s.frameworks = 'AVFoundation', 'CoreMedia', 'CryptoKit', 'Network', 'Security', 'VideoToolbox', 'Vision'

  s.pod_target_xcconfig = {
    'DEFINES_MODULE' => 'YES',
    'SWIFT_COMPILATION_MODE' => 'wholemodule'
  }

  s.source_files = '**/*.{h,m,swift}'
end
