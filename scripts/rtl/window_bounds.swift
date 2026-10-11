// Prints "x y width height" (points) of the on-screen window whose number is
// the argument. Used by kitty_shots.py to crop a full-screen capture, because
// `screencapture -l` returns a black image for kitty windows on this machine.
import CoreGraphics
import Foundation

let wanted = Int(CommandLine.arguments[1]) ?? -1
let wins = CGWindowListCopyWindowInfo([.optionAll], kCGNullWindowID) as! [[String: Any]]
for w in wins where (w[kCGWindowNumber as String] as? Int) == wanted {
  let b = w[kCGWindowBounds as String] as! [String: Double]
  print("\(Int(b["X"]!)) \(Int(b["Y"]!)) \(Int(b["Width"]!)) \(Int(b["Height"]!))")
  exit(0)
}
exit(1)
