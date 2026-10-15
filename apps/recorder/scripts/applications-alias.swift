// Makes the "Applications" drop target of the .dmg: a Finder alias to /Applications with the folder's icon written on
// it. A plain symlink (what hdiutil scripts usually use) shows up blank in recent Finder versions.
//   swift applications-alias.swift <stage dir>
import AppKit

let dir = URL(fileURLWithPath: CommandLine.arguments[1])
let alias = dir.appendingPathComponent("Applications")
let target = URL(fileURLWithPath: "/Applications")
let data = try target.bookmarkData(options: .suitableForBookmarkFile, includingResourceValuesForKeys: nil, relativeTo: nil)
try URL.writeBookmarkData(data, to: alias)
let icon = NSWorkspace.shared.icon(forFile: target.path)
guard NSWorkspace.shared.setIcon(icon, forFile: alias.path, options: []) else {
    FileHandle.standardError.write("could not set the Applications icon\n".data(using: .utf8)!)
    exit(1)
}
