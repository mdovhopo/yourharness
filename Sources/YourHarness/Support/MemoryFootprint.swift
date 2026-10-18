import Darwin

/// The same "Memory" number Activity Monitor shows for this process.
enum MemoryFootprint {
    static func current() -> UInt64 {
        var usage = rusage_info_v4()
        let result = withUnsafeMutablePointer(to: &usage) {
            $0.withMemoryRebound(to: rusage_info_t?.self, capacity: 1) {
                proc_pid_rusage(getpid(), RUSAGE_INFO_V4, $0)
            }
        }
        return result == 0 ? usage.ri_phys_footprint : 0
    }

    static var formatted: String {
        current().formatted(.byteCount(style: .memory))
    }
}
