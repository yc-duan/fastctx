import AppKit
import WebKit

let output = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
try FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)
final class Probe: NSObject, NSApplicationDelegate, WKNavigationDelegate, WKScriptMessageHandler {
    var window: NSWindow!
    var web: WKWebView!
    var control = true
    var records: [[String: Any]] = []
    func applicationDidFinishLaunching(_ notification: Notification) {
        let config = WKWebViewConfiguration()
        config.userContentController.add(self, name: "evidence")
        web = WKWebView(frame: NSRect(x: 0, y: 0, width: 1000, height: 650), configuration: config)
        web.navigationDelegate = self
        window = NSWindow(contentRect: web.frame, styleMask: [.titled, .closable, .resizable], backing: .buffered, defer: false)
        window.title = "WKWebView rendering control"
        window.contentView = web
        window.center()
        window.makeKeyAndOrderFront(nil)
        NSApplication.shared.activate(ignoringOtherApps: true)
        web.loadHTMLString("<html><body style='margin:0;background:#ffcc00;color:black;font:32px sans-serif'><h1>Simple paint control</h1><p>No application CSS or scripts.</p><div style='background:#00aaff;height:150px'>Blue rectangle</div></body></html>", baseURL: nil)
        DispatchQueue.main.asyncAfter(deadline: .now() + 150) { NSApplication.shared.terminate(nil) }
    }
    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        if control {
            control = false
            DispatchQueue.main.asyncAfter(deadline: .now() + 2) {
                self.capture("simple", state: ["source": "plain HTML without application assets"]) {
                    self.web.load(URLRequest(url: URL(string: "http://127.0.0.1:5317/?s=claude&conversations&theme=warm-white")!))
                }
            }
        } else {
            let script = #"""
            (async () => {
              const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
              for (let i=0; i<150 && !document.querySelector('.sp-toggle'); i++) await delay(100);
              localStorage.removeItem('side-open');
              localStorage.setItem('motion','always');
              const click = selector => { const element=document.querySelector(selector); if(!element)throw Error(selector); element.click(); };
              const measure = () => {
                const result={pressed:document.querySelector('.sp-toggle').getAttribute('aria-pressed'),dock:document.querySelector('.sp-split').dataset.dock,agent:navigator.userAgent};
                for(const selector of ['.sp-host','.sp-panel','.sp-sheet']) {
                  const element=document.querySelector(selector), style=getComputedStyle(element), rect=element.getBoundingClientRect();
                  result[selector]={classes:element.className,hidden:element.hidden,width:rect.width,height:rect.height,opacity:style.opacity};
                }
                return result;
              };
              const capture = label => new Promise(resolve => { window.captureDone=resolve; window.webkit.messageHandlers.evidence.postMessage({label,state:measure()}); });
              const observe = async (label, action, wantOpen, dock) => {
                action(); await delay(3300);
                const state=measure(), panel=state['.sp-panel'], sheet=state['.sp-sheet'];
                state.passed=wantOpen ? state.pressed==='true' && state.dock===dock && !panel.hidden && panel.width>100 && panel.height>100 && +sheet.opacity===1 : state.pressed==='false' && panel.hidden;
                await new Promise(resolve => { window.captureDone=resolve; window.webkit.messageHandlers.evidence.postMessage({label,state}); });
              };
              await delay(2500);
              if(document.querySelector('.sp-toggle').getAttribute('aria-pressed')==='true'){click('.sp-toggle');await delay(1000);}
              for(const dock of ['right','bottom','left']) {
                if(dock!=='right') {
                  click('.sp-toggle'); click('.sp-more'); await delay(100);
                  await observe(dock+'-dock',()=>click('.sp-menu-row[data-dock="'+dock+'"]'),true,dock);
                  await observe(dock+'-dock-close',()=>click('.sp-toggle'),false,dock);
                }
                for(let i=0;i<2;i++) {
                  await observe(dock+'-open-'+i,()=>click('.sp-toggle'),true,dock);
                  await observe(dock+'-close-'+i,()=>click('.sp-toggle'),false,dock);
                }
              }
              const style=document.createElement('style');style.textContent='*,*::before,*::after{transition:none!important;animation:none!important}';document.head.append(style);
              await observe('no-animation-open',()=>click('.sp-toggle'),true,'left');
              await observe('no-animation-close',()=>click('.sp-toggle'),false,'left');
              window.webkit.messageHandlers.evidence.postMessage({label:'done',state:{complete:true}});
            })().catch(error => window.webkit.messageHandlers.evidence.postMessage({label:'error',state:{error:String(error)}}));
            """#
            web.evaluateJavaScript(script, completionHandler: nil)
        }
    }
    func userContentController(_ userContentController: WKUserContentController, didReceive message: WKScriptMessage) {
        guard let body = message.body as? [String: Any], let label = body["label"] as? String, let state = body["state"] as? [String: Any] else { return }
        if label == "done" || label == "error" {
            records.append(body)
            save()
            NSApplication.shared.terminate(nil)
            return
        }
        capture(label, state: state) { self.web.evaluateJavaScript("window.captureDone()", completionHandler: nil) }
    }
    func save() {
        if let data = try? JSONSerialization.data(withJSONObject: records, options: [.prettyPrinted, .sortedKeys]) {
            try? data.write(to: output.appendingPathComponent("results.json"))
        }
    }
    func capture(_ label: String, state: [String: Any], completion: @escaping () -> Void) {
        records.append(["label": label, "state": state])
        save()
        web.takeSnapshot(with: nil) { image, error in
            if let tiff = image?.tiffRepresentation, let bitmap = NSBitmapImageRep(data: tiff), let png = bitmap.representation(using: .png, properties: [:]) {
                try? png.write(to: output.appendingPathComponent(label + ".png"))
            }
            let process = Process()
            process.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture")
            process.arguments = ["-x", output.appendingPathComponent("native-" + label + ".png").path]
            try? process.run()
            process.waitUntilExit()
            print(label, state, "snapshotError", String(describing: error))
            completion()
        }
    }
}
let app = NSApplication.shared
let delegate = Probe()
app.setActivationPolicy(.regular)
app.delegate = delegate
app.run()
