import Foundation
import Testing
@testable import HarnessCore

struct LineBufferTests {
    @Test func splitsAcrossChunks() {
        var buffer = LineBuffer()
        #expect(buffer.append(Data("{\"a\":1}\n{\"b\"".utf8)) == [Data("{\"a\":1}".utf8)])
        #expect(buffer.append(Data(":2}\n\n{\"c\":3}\n".utf8)) == [Data("{\"b\":2}".utf8), Data("{\"c\":3}".utf8)])
        #expect(buffer.append(Data()).isEmpty)
    }

    /// A screenshot's line: megabytes in pipe-sized chunks.
    @Test func longLineInSmallChunks() {
        var buffer = LineBuffer()
        let line = Data(repeating: UInt8(ascii: "x"), count: 4_000_000)
        var lines: [Data] = []
        for start in stride(from: 0, to: line.count, by: 65_536) {
            lines += buffer.append(line[start..<min(start + 65_536, line.count)])
        }
        lines += buffer.append(Data("\n".utf8))
        #expect(lines == [line])
    }
}
