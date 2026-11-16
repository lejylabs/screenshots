// Explicit macOS CI/operator fixture. Public APIs only; never requests TCC access.
import AppKit
import ApplicationServices
import CoreGraphics
import Foundation

func emit(_ value: [String: Any]) {
    let bytes = try! JSONSerialization.data(withJSONObject: value, options: [.sortedKeys])
    FileHandle.standardOutput.write(bytes)
    FileHandle.standardOutput.write(Data([10]))
}

guard ProcessInfo.processInfo.environment["SCREENSHOTS_OWNED_MAC_FIXTURE"] == "1" else {
    emit(["error": "explicit owned fixture opt-in required"])
    exit(2)
}
guard CGPreflightScreenCaptureAccess() else {
    emit(["screen_recording": false, "accessibility": AXIsProcessTrusted(), "requested_permission": false])
    exit(77)
}

final class Document: NSView {
    override var isFlipped: Bool { true }
    override var isOpaque: Bool { false }
    override func draw(_ dirtyRect: NSRect) {
        let first = max(0, Int(floor(dirtyRect.minY)))
        let last = min(Int(bounds.height), Int(ceil(dirtyRect.maxY)))
        for row in first..<last {
            // Nine row bits across three primary-color stripes identify 512 rows.
            // Primaries also make the fixed RGB8 GIF palette oracle exact.
            for column in 0..<Int(ceil(bounds.width / 10)) {
                if row >= 40 && row < 60 && column < 2 { continue }
                let bits = (row >> ((column % 3) * 3)) & 7
                NSColor(srgbRed: bits & 4 == 0 ? 0 : 1, green: bits & 2 == 0 ? 0 : 1,
                        blue: bits & 1 == 0 ? 0 : 1, alpha: 1).setFill()
                NSRect(x: CGFloat(column * 10), y: CGFloat(row), width: 10, height: 1).fill()
            }
            if row >= 40 && row < 60 {
                NSColor(srgbRed: 1, green: 0, blue: 0, alpha: 0.5).setFill()
                NSRect(x: 0, y: CGFloat(row), width: 20, height: 1).fill()
            }
        }
    }
    override func resetCursorRects() { addCursorRect(bounds, cursor: .crosshair) }
}

let app = NSApplication.shared
app.setActivationPolicy(.accessory)
let window = NSWindow(contentRect: NSRect(x: 120, y: 160, width: 340, height: 240),
                      styleMask: [.titled, .closable, .resizable], backing: .buffered, defer: false)
window.title = "Owned capture fixture"
window.isReleasedWhenClosed = false
window.hasShadow = true
window.isOpaque = false
window.backgroundColor = .clear
let scroll = NSScrollView(frame: NSRect(x: 0, y: 0, width: 340, height: 240))
scroll.hasVerticalScroller = true
scroll.scrollerStyle = .legacy
scroll.borderType = .noBorder
scroll.drawsBackground = false
scroll.contentView.drawsBackground = false
let document = Document(frame: NSRect(x: 0, y: 0, width: 340, height: 1200))
scroll.documentView = document
window.contentView = scroll
// Move only the owned window beneath the unchanged pointer, never the pointer.
let pointer = NSEvent.mouseLocation
if let screen = NSScreen.screens.first(where: { $0.frame.contains(pointer) }) {
    let available = screen.visibleFrame
    window.setFrameOrigin(NSPoint(x: min(max(pointer.x - 170, available.minX), available.maxX - window.frame.width),
                                  y: min(max(pointer.y - 120, available.minY), available.maxY - window.frame.height)))
}
window.makeKeyAndOrderFront(nil)
app.activate(ignoringOtherApps: true)
NSCursor.crosshair.set()

DispatchQueue.main.asyncAfter(deadline: .now() + .milliseconds(200)) {
    guard let screen = window.screen,
          let number = screen.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber else {
        emit(["error": "owned fixture display unavailable"])
        app.terminate(nil)
        return
    }
    let scale = screen.backingScaleFactor
    let content = window.contentRect(forFrameRect: window.frame)
    let topInset = window.frame.maxY - content.maxY
    let region = [Int((content.minX-window.frame.minX+30)*scale), Int((topInset+5)*scale),
                  Int((content.width-55)*scale), Int((content.height-10)*scale)]
    let displayRegion = [Int((content.minX-screen.frame.minX+30)*scale),
                         Int((screen.frame.maxY-content.maxY+5)*scale), region[2], region[3]]
    let cursor = NSCursor.crosshair
    let pointerNow = NSEvent.mouseLocation
    let cursorROI = [Int((pointerNow.x-content.minX-30)*scale), Int((content.maxY-pointerNow.y-5)*scale)]
    emit(["window": window.windowNumber, "pid": ProcessInfo.processInfo.processIdentifier,
          "display": number.uint32Value, "scale": scale, "region": region,
          "display_region": displayRegion,
          "document_height": Int(document.bounds.height),
          "window_size": [Int(window.frame.width*scale), Int(window.frame.height*scale)],
          "alpha_pixel": [Int((content.minX-window.frame.minX+10)*scale), Int((topInset+45)*scale)],
          "cursor_roi": cursorROI,
          "cursor_size": [Int(cursor.image.size.width*scale), Int(cursor.image.size.height*scale)],
          "cursor_hotspot": [Int(cursor.hotSpot.x*scale), Int(cursor.hotSpot.y*scale)],
          "screen_recording": true, "accessibility": AXIsProcessTrusted(),
          "requested_permission": false])
}

// The private parent pipe controls only this owned fixture; it sends no OS input.
DispatchQueue.global(qos: .utility).async {
    while let command = readLine() {
        DispatchQueue.main.async {
            if command == "move" { window.setFrameOrigin(NSPoint(x: window.frame.minX+20, y: window.frame.minY)) }
            if command == "focus" { window.makeKeyAndOrderFront(nil); app.activate(ignoringOtherApps: true) }
            if command == "reset" {
                document.scroll(NSPoint.zero)
                scroll.reflectScrolledClipView(scroll.contentView)
                document.displayIfNeeded()
            }
            if command == "offset" {
                emit(["offset_points": scroll.contentView.bounds.origin.y])
            }
            if command == "quit" { app.terminate(nil) }
        }
    }
    DispatchQueue.main.async { app.terminate(nil) }
}
app.run()
