// Prints "x y width height" (points) of the on-screen window whose number is
// the argument. Used by kitty_shots.py to crop a full-screen capture, because
// `screencapture -l` returns a black image for kitty windows on this machine.
//
// A screen capture contains whatever is in front of the window, so this also
// refuses (exit 2, owner names on stderr) when another normal window overlaps
// it: better no picture than a picture of somebody else's content.
import CoreGraphics
import Foundation

let wanted = Int(CommandLine.arguments[1]) ?? -1
// Front to back.
let wins = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as! [[String: Any]]

func rect(_ w: [String: Any]) -> CGRect {
  let b = w[kCGWindowBounds as String] as! [String: Double]
  return CGRect(x: b["X"]!, y: b["Y"]!, width: b["Width"]!, height: b["Height"]!)
}

guard let ours = wins.first(where: { ($0[kCGWindowNumber as String] as? Int) == wanted }) else {
  exit(1)
}
let target = rect(ours)
var covering: [String] = []
for w in wins {
  if (w[kCGWindowNumber as String] as? Int) == wanted { break }
  let layer = w[kCGWindowLayer as String] as? Int ?? 0
  let alpha = w[kCGWindowAlpha as String] as? Double ?? 1
  if layer == 0 && alpha > 0 && rect(w).intersects(target) {
    covering.append(w[kCGWindowOwnerName as String] as? String ?? "?")
  }
}
if !covering.isEmpty {
  FileHandle.standardError.write("covered by: \(Set(covering).sorted().joined(separator: ", "))\n".data(using: .utf8)!)
  exit(2)
}
print("\(Int(target.minX)) \(Int(target.minY)) \(Int(target.width)) \(Int(target.height))")
