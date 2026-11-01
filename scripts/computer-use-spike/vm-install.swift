import Foundation
import Virtualization
setvbuf(stdout, nil, _IOLBF, 0)
let d = URL(fileURLWithPath: NSHomeDirectory() + "/.yh-vm-spike")
let start = Date()
func t() -> String { String(format: "%.0fs", Date().timeIntervalSince(start)) }
var keep: [Any] = []
VZMacOSRestoreImage.load(from: d.appendingPathComponent("restore.ipsw")) { r in
  guard case .success(let image) = r, let req = image.mostFeaturefulSupportedConfiguration else { print("load failed", r); exit(1) }
  DispatchQueue.main.async {
    do {
      let p = VZMacPlatformConfiguration()
      p.hardwareModel = req.hardwareModel
      let id = VZMacMachineIdentifier()
      p.machineIdentifier = id
      p.auxiliaryStorage = try VZMacAuxiliaryStorage(creatingStorageAt: d.appendingPathComponent("aux.img"), hardwareModel: req.hardwareModel, options: [.allowOverwrite])
      try req.hardwareModel.dataRepresentation.write(to: d.appendingPathComponent("hw.bin"))
      try id.dataRepresentation.write(to: d.appendingPathComponent("id.bin"))
      let disk = d.appendingPathComponent("disk.img")
      FileManager.default.createFile(atPath: disk.path, contents: nil)
      let h = try FileHandle(forWritingTo: disk); try h.truncate(atOffset: 60 << 30); try h.close()
      let c = VZVirtualMachineConfiguration()
      c.platform = p; c.bootLoader = VZMacOSBootLoader()
      c.cpuCount = 4; c.memorySize = 8 << 30
      let g = VZMacGraphicsDeviceConfiguration(); g.displays = [VZMacGraphicsDisplayConfiguration(widthInPixels: 1920, heightInPixels: 1200, pixelsPerInch: 144)]
      c.graphicsDevices = [g]
      c.keyboards = [VZUSBKeyboardConfiguration()]; c.pointingDevices = [VZUSBScreenCoordinatePointingDeviceConfiguration()]
      let n = VZVirtioNetworkDeviceConfiguration(); n.attachment = VZNATNetworkDeviceAttachment(); c.networkDevices = [n]
      c.storageDevices = [VZVirtioBlockDeviceConfiguration(attachment: try VZDiskImageStorageDeviceAttachment(url: disk, readOnly: false))]
      try c.validate()
      let vm = VZVirtualMachine(configuration: c)
      let inst = VZMacOSInstaller(virtualMachine: vm, restoringFromImageAt: d.appendingPathComponent("restore.ipsw"))
      let obs = inst.progress.observe(\.fractionCompleted) { pr, _ in print(t(), Int(pr.fractionCompleted * 100), "%") }
      keep = [vm, inst, obs]
      print(t(), "installing, req cpu", req.minimumSupportedCPUCount)
      inst.install { res in print(t(), "INSTALL", res); exit({ if case .success = res { return 0 }; return 1 }()) }
    } catch { print("setup failed", error); exit(1) }
  }
}
dispatchMain()
