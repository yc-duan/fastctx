// Drives the Fastctx window on a macOS runner as a person would: where it
// is, clicks, a drag, a key with Command, and where the window's red button
// is drawn. Compiled once with swiftc; each call does one thing.
//
//   walk trusted | window | click X Y | wobble X Y | drag X1 Y1 X2 Y2 | key m|w|q | red

import AppKit
import ApplicationServices
import CoreGraphics

func fastctxWindows() -> [CGRect] {
  let all = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID)
    as? [[String: Any]] ?? []
  return all.compactMap { info in
    guard let owner = info[kCGWindowOwnerName as String] as? String,
      owner.lowercased().contains("fastctx"),
      (info[kCGWindowLayer as String] as? Int) == 0,
      let bounds = info[kCGWindowBounds as String] as? [String: CGFloat],
      let width = bounds["Width"], width > 100
    else { return nil }
    return CGRect(x: bounds["X"]!, y: bounds["Y"]!, width: width, height: bounds["Height"]!)
  }
}

let source = CGEventSource(stateID: .hidSystemState)

func post(_ type: CGEventType, _ at: CGPoint) {
  CGEvent(mouseEventSource: source, mouseType: type, mouseCursorPosition: at, mouseButton: .left)?
    .post(tap: .cghidEventTap)
}

func click(_ at: CGPoint, wobble: Bool) {
  post(.mouseMoved, at)
  usleep(150_000)
  post(.leftMouseDown, at)
  if wobble {
    // A finger on a trackpad moves a little while it presses.
    for step in 1...3 {
      usleep(20_000)
      post(.leftMouseDragged, CGPoint(x: at.x + CGFloat(step), y: at.y + CGFloat(step) / 2))
    }
  }
  usleep(80_000)
  post(.leftMouseUp, wobble ? CGPoint(x: at.x + 3, y: at.y + 1.5) : at)
}

func drag(_ from: CGPoint, _ to: CGPoint) {
  post(.mouseMoved, from)
  usleep(150_000)
  post(.leftMouseDown, from)
  usleep(150_000)
  for step in 1...20 {
    let t = CGFloat(step) / 20
    post(.leftMouseDragged, CGPoint(x: from.x + (to.x - from.x) * t, y: from.y + (to.y - from.y) * t))
    usleep(25_000)
  }
  usleep(150_000)
  post(.leftMouseUp, to)
}

func key(_ code: CGKeyCode) {
  for down in [true, false] {
    let event = CGEvent(keyboardEventSource: source, virtualKey: code, keyDown: down)
    event?.flags = .maskCommand
    event?.post(tap: .cghidEventTap)
    usleep(60_000)
  }
}

/// The red button's centre, found by its colour near the window's top left.
func red(in window: CGRect) -> CGPoint? {
  guard let image = CGDisplayCreateImage(CGMainDisplayID()) else { return nil }
  let scale = CGFloat(image.width) / CGDisplayBounds(CGMainDisplayID()).width
  let rep = NSBitmapImageRep(cgImage: image)
  var found: [CGPoint] = []
  for y in stride(from: window.minY, to: window.minY + 60, by: 1) {
    for x in stride(from: window.minX, to: window.minX + 60, by: 1) {
      guard let color = rep.colorAt(x: Int(x * scale), y: Int(y * scale)) else { continue }
      if color.redComponent > 0.85 && color.greenComponent < 0.5 && color.blueComponent < 0.5 {
        found.append(CGPoint(x: x, y: y))
      }
    }
  }
  guard !found.isEmpty else { return nil }
  let sum = found.reduce(CGPoint.zero) { CGPoint(x: $0.x + $1.x, y: $0.y + $1.y) }
  return CGPoint(x: sum.x / CGFloat(found.count), y: sum.y / CGFloat(found.count))
}

let args = CommandLine.arguments.dropFirst().map { $0 }
let numbers = args.dropFirst().compactMap { Double($0) }.map { CGFloat($0) }
switch args.first {
case "trusted":
  print(AXIsProcessTrusted() ? "trusted" : "not trusted")
case "window":
  let windows = fastctxWindows()
  print(windows.map { "\(Int($0.minX)) \(Int($0.minY)) \(Int($0.width)) \(Int($0.height))" }.joined(separator: "\n"))
  exit(windows.isEmpty ? 1 : 0)
case "click":
  click(CGPoint(x: numbers[0], y: numbers[1]), wobble: false)
case "wobble":
  click(CGPoint(x: numbers[0], y: numbers[1]), wobble: true)
case "drag":
  drag(CGPoint(x: numbers[0], y: numbers[1]), CGPoint(x: numbers[2], y: numbers[3]))
case "key":
  key(["m": 0x2E, "w": 0x0D, "q": 0x0C][args[1]]!)
case "red":
  guard let window = fastctxWindows().first, let at = red(in: window) else { exit(1) }
  print("\(Int(at.x)) \(Int(at.y))")
default:
  FileHandle.standardError.write("unknown step\n".data(using: .utf8)!)
  exit(2)
}
