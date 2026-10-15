// The recorder's Parakeet engine, as a long-running helper: the models load once, then turns come and go over
// stdin/stdout.
//
//   recordem-parakeet --model v3|ultra|redux --models <dir>
//
// Once loaded it writes one line: {"ready": true, "model": "v3"}. Then, per turn, it reads a header line
// {"samples": N, "language": "pt"} followed by N little-endian Float32 samples (16 kHz mono), and writes one line:
// {"text": "...", "words": [{"text": "Olá", "start_ms": 120, "end_ms": 480}, ...]} or {"error": "..."}.
// Times are from the start of the turn. EOF on stdin ends it.
import FluidAudio
import Foundation

struct Header: Decodable {
    let samples: Int
    let language: String?
}

struct Word: Encodable {
    let text: String
    let start_ms: Int
    let end_ms: Int
}

struct Reply: Encodable {
    var ready: Bool? = nil
    var model: String? = nil
    var text: String? = nil
    var words: [Word]? = nil
    var error: String? = nil
}

func send(_ r: Reply) {
    let data = (try? JSONEncoder().encode(r)) ?? Data("{\"error\":\"encode\"}".utf8)
    FileHandle.standardOutput.write(data)
    FileHandle.standardOutput.write(Data("\n".utf8))
}

func fail(_ message: String) -> Never {
    send(Reply(error: message))
    exit(1)
}

/// Reads exactly `count` bytes, or nil at EOF.
func read(_ count: Int) -> Data? {
    var out = Data()
    while out.count < count {
        let chunk = FileHandle.standardInput.readData(ofLength: count - out.count)
        if chunk.isEmpty { return nil }
        out.append(chunk)
    }
    return out
}

func readLine() -> String? {
    var line = Data()
    while true {
        let byte = FileHandle.standardInput.readData(ofLength: 1)
        if byte.isEmpty { return line.isEmpty ? nil : String(decoding: line, as: UTF8.self) }
        if byte[0] == 0x0A { return String(decoding: line, as: UTF8.self) }
        line.append(byte)
    }
}

var args = CommandLine.arguments.dropFirst()
var modelName = "v3"
var modelsDir: URL? = nil
while let a = args.popFirst() {
    switch a {
    case "--model": modelName = args.popFirst() ?? modelName
    case "--models": modelsDir = args.popFirst().map { URL(fileURLWithPath: $0, isDirectory: true) }
    default: fail("unknown argument \(a)")
    }
}
let version: AsrModelVersion
switch modelName {
case "v3": version = .v3
case "ultra": version = .ultra
case "redux": version = .redux
default: fail("unknown model \(modelName)")
}

let models: AsrModels
do {
    models = try await AsrModels.downloadAndLoad(to: modelsDir, version: version)
} catch {
    fail("loading \(modelName): \(error)")
}
let asr = AsrManager(models: models)
let layers = await asr.decoderLayerCount
send(Reply(ready: true, model: modelName))

while let line = readLine() {
    guard let header = try? JSONDecoder().decode(Header.self, from: Data(line.utf8)) else {
        fail("bad header: \(line)")
    }
    guard let bytes = read(header.samples * 4) else { break }
    let samples: [Float] = bytes.withUnsafeBytes { raw in
        (0..<header.samples).map { Float(bitPattern: UInt32(littleEndian: raw.loadUnaligned(fromByteOffset: $0 * 4, as: UInt32.self))) }
    }
    do {
        // Each turn on its own, like Whisper with no context: a turn never leans on another voice's words.
        var state = TdtDecoderState.make(decoderLayers: layers)
        let language = header.language.flatMap { Language(rawValue: $0) }
        let result = try await asr.transcribe(samples, decoderState: &state, language: language)
        let words = buildWordTimings(from: result.tokenTimings ?? []).map {
            Word(text: $0.word, start_ms: Int($0.startTime * 1000), end_ms: Int($0.endTime * 1000))
        }
        send(Reply(text: result.text, words: words))
    } catch {
        send(Reply(error: "\(error)"))
    }
}
