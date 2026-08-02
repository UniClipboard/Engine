require "fileutils"
require "xcodeproj"

output = ARGV.fetch(0)
platform = ARGV.fetch(1, "device")
unless %w[device simulator].include?(platform)
  abort "platform must be device or simulator"
end
rust_target = platform == "simulator" ? "aarch64-apple-ios-sim" : "aarch64-apple-ios"
supported_platform = platform == "simulator" ? "iphonesimulator" : "iphoneos"
archive = "$(SRCROOT)/../../target/ios-probe-cargo/#{rust_target}/release/libuc_mobile_probe_core.a"
FileUtils.rm_rf(output)
project = Xcodeproj::Project.new(output)
target = project.new_target(:application, "EngineProbe", :ios, "17.0")

source_group = project.main_group.new_group(
  "EngineProbe",
  "../../tests/hosts/ios/EngineProbe"
)
%w[
  EngineProbeApp.swift
  ProbeBridge.swift
  ProbeModel.swift
  ProbeView.swift
].each do |name|
  target.source_build_phase.add_file_reference(source_group.new_file(name))
end
source_group.new_file("Info.plist")
source_group.new_file("EngineProbe-Bridging-Header.h")
source_group.new_file("uc_ios_probe.h")

target.build_configurations.each do |config|
  settings = config.build_settings
  settings["PRODUCT_BUNDLE_IDENTIFIER"] = "app.uniclipboard.EngineProbe"
  settings["DEVELOPMENT_TEAM"] = "8XG39X5CL8"
  settings["CODE_SIGN_STYLE"] = "Automatic"
  settings["SWIFT_VERSION"] = "5.0"
  settings["INFOPLIST_FILE"] = "../../tests/hosts/ios/EngineProbe/Info.plist"
  settings["SWIFT_OBJC_BRIDGING_HEADER"] = "../../tests/hosts/ios/EngineProbe/EngineProbe-Bridging-Header.h"
  settings["LIBRARY_SEARCH_PATHS"] = ["$(inherited)", File.dirname(archive)]
  settings["OTHER_LDFLAGS"] = [
    "$(inherited)", "-force_load",
    archive,
    "-framework", "Security",
    "-framework", "SystemConfiguration", "-framework", "CoreFoundation",
    "-framework", "Network",
    "-lsqlite3", "-lz", "-liconv", "-lresolv",
  ]
  settings["TARGETED_DEVICE_FAMILY"] = "1"
  settings["SUPPORTED_PLATFORMS"] = supported_platform
  settings["SUPPORTS_MACCATALYST"] = "NO"
end

project.save
