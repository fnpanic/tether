// Mac side of mac-webview-drag-e2e.mjs: a floating window with a WKWebView text area whose text
// starts at the screen fraction (0.2, 0.3). Prints what the web view saw of a drag made through
// Tether: the AppKit events around the press (a plain move between press and release, MOVED-AFTER-PRESS,
// is what made WebKit drop the selection) and "JS SELECTED <characters>" after each release.
//   swiftc -O mac-webview-drag.swift -o wkdrag && ./wkdrag [seconds, default 60]
import AppKit
import WebKit

setbuf(stdout, nil)
let seconds = CommandLine.arguments.count > 1 ? Double(CommandLine.arguments[1]) ?? 60 : 60

class Log: NSObject, WKScriptMessageHandler {
    func userContentController(_ u: WKUserContentController, didReceive m: WKScriptMessage) { print("JS", m.body) }
}
let app = NSApplication.shared
app.setActivationPolicy(.regular)
let screen = NSScreen.main!.frame
let win = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 700, height: 300), styleMask: [.titled], backing: .buffered, defer: false)
win.level = .floating // Tether's own window comes to the front when a session connects
// The text area's first characters (40 pt into the content) at (0.2, 0.3) of the screen.
win.setFrameOrigin(NSPoint(x: screen.width * 0.2 - 40, y: screen.height * 0.7 + 40 - win.contentLayoutRect.height))
let cfg = WKWebViewConfiguration()
let log = Log()
cfg.userContentController.add(log, name: "log")
let wv = WKWebView(frame: win.contentView!.bounds, configuration: cfg)
win.contentView!.addSubview(wv)
wv.loadHTMLString("""
<body style="margin:0"><textarea id=t style="width:680px;height:280px;font:20px monospace">\(String(repeating: "The quick brown fox jumps over the lazy dog. ", count: 12))</textarea>
<script>const L=m=>webkit.messageHandlers.log.postMessage(m);
for(const k of ['mousedown','mouseup']) addEventListener(k,e=>L(k+' buttons='+e.buttons+' '+e.clientX+','+e.clientY),true);
addEventListener('mouseup',()=>setTimeout(()=>{const n=t.selectionEnd-t.selectionStart;if(n)L('SELECTED '+n)},200));</script></body>
""", baseURL: nil)
win.makeKeyAndOrderFront(nil)
app.activate(ignoringOtherApps: true)

var drags = 0, down = false
NSEvent.addLocalMonitorForEvents(matching: [.leftMouseDown, .leftMouseUp, .leftMouseDragged, .mouseMoved]) { e in
    if e.type == .leftMouseDown { down = true; drags = 0 }
    if e.type == .leftMouseUp { down = false }
    if e.type == .mouseMoved && !down { return e }
    if e.type == .leftMouseDragged { drags += 1; if drags > 2 { return e } }
    let name = [NSEvent.EventType.leftMouseDown: "down", .leftMouseUp: "up", .leftMouseDragged: "dragged", .mouseMoved: "MOVED-AFTER-PRESS"][e.type] ?? "?"
    print("NS \(name) t=\(e.timestamp) at \(e.locationInWindow)")
    return e
}
DispatchQueue.main.asyncAfter(deadline: .now() + seconds) { print("END"); exit(0) }
app.run()
