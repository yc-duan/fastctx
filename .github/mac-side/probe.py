"""Compare the side panel in two source revisions in real macOS Safari."""
import argparse
import base64
import json
import pathlib
import platform
import subprocess
import time
import urllib.request

parser = argparse.ArgumentParser()
parser.add_argument("--out", required=True)
parser.add_argument("--diagnose", action="store_true")
args = parser.parse_args()
out = pathlib.Path(args.out)
out.mkdir(parents=True, exist_ok=True)
endpoint = "http://127.0.0.1:4444"
session = None
results = []


def request(method, path, body=None):
    payload = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(endpoint + path, data=payload, method=method, headers={"Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=75) as response:
            value = json.load(response).get("value")
    except urllib.error.HTTPError as error:
        raise RuntimeError(error.read().decode()) from error
    if isinstance(value, dict) and "error" in value:
        raise RuntimeError(value)
    return value


def command(method, path, body=None):
    return request(method, f"/session/{session}{path}", body)


def js(script, arguments=None):
    return command("POST", "/execute/sync", {"script": script, "args": arguments or []})


def click(selector):
    element = command("POST", "/element", {"using": "css selector", "value": selector})
    key = element["element-6066-11e4-a52e-4f735466cecf"]
    command("POST", f"/element/{key}/click", {})


def wait_for(script):
    for _ in range(150):
        if js(script):
            return
        time.sleep(0.1)
    raise RuntimeError("Page did not become ready: " + script)


MEASURE = r"""
const nodes = ['.sp-host', '.sp-split', '.sp-panel', '.sp-sheet'];
const result = {at:performance.now(), pressed:document.querySelector('.sp-toggle')?.getAttribute('aria-pressed'), dock:document.querySelector('.sp-split')?.dataset.dock, reduced:document.documentElement.hasAttribute('data-reduced')};
for (const selector of nodes) {
  const element=document.querySelector(selector);
  if(!element){result[selector]=null;continue;}
  const style=getComputedStyle(element), rect=element.getBoundingClientRect();
  result[selector]={width:rect.width,height:rect.height,x:rect.x,y:rect.y,hidden:element.hidden,classes:element.className,open:style.getPropertyValue('--sp-open').trim(),sideWidth:style.getPropertyValue('--side-width').trim(),sideHeight:style.getPropertyValue('--side-height').trim(),opacity:style.opacity,display:style.display,transition:style.transition};
}
return result;
"""


def opened(state, dock):
    panel, sheet = state[".sp-panel"], state[".sp-sheet"]
    return (state["pressed"] == "true" and state["dock"] == dock and not panel["hidden"]
            and panel["width"] > 100 and panel["height"] > 100 and sheet["width"] > 100
            and abs(float(sheet["opacity"]) - 1) < 0.001
            and "sp-shown" in state[".sp-host"]["classes"].split())


def observe(label, action, want_open, dock, duration=3.3):
    js("window.sideTrace=[];window.sideTraceStart=performance.now();window.sideTraceUntil=performance.now()+arguments[0];const measure=new Function(arguments[1]);function frame(){window.sideTrace.push(measure());if(performance.now()<window.sideTraceUntil)requestAnimationFrame(frame);}requestAnimationFrame(frame);", [30000, MEASURE])
    action()
    action_ended = js("window.sideTraceUntil=performance.now()+arguments[0];return performance.now();", [duration * 1000 + 150])
    started = time.monotonic()
    shots = []
    times = (0.08, 0.35, 0.8, 1.3, duration) if want_open else (0.08, duration)
    for target in times:
        time.sleep(max(0, target - (time.monotonic() - started)))
        state = js(MEASURE)
        image = command("GET", "/screenshot")
        name = f"{label}-{round(target * 1000):04d}.png"
        (out / name).write_bytes(base64.b64decode(image))
        shots.append({"requestedSeconds": target, "elapsedSeconds": time.monotonic() - started, "file": name, "state": state})
        if args.diagnose and target == duration:
            capture = subprocess.run(["screencapture", "-x", str(out / ("native-" + name))], capture_output=True, text=True, timeout=10)
            shots[-1]["nativeCapture"] = {"code": capture.returncode, "error": capture.stderr}
    trace = js("return window.sideTrace")
    state = js(MEASURE)
    if args.diagnose:
        state["ancestors"] = js("const rows=[];for(let e=document.querySelector('.sp-sheet');e;e=e.parentElement){const s=getComputedStyle(e);rows.push({tag:e.tagName,classes:e.className,opacity:s.opacity,visibility:s.visibility,filter:s.filter,transform:s.transform,shown:s.getPropertyValue('--shown'),text:e.textContent.length});}return rows;")
    settled = [item for item in trace if item["at"] - action_ended >= 900]
    passed = opened(state, dock) and all(opened(item, dock) for item in settled) if want_open else state["pressed"] == "false" and state[".sp-panel"]["hidden"]
    item = {"label": label, "actionEnded": action_ended, "wantOpen": want_open, "passed": passed, "final": state, "trace": trace, "shots": shots}
    results.append(item)
    (out / "results.json").write_text(json.dumps({"metadata": metadata, "cases": results}, indent=2))
    print(label, "PASS" if passed else "FAIL", json.dumps(state), flush=True)
    return passed


metadata = {"platform": platform.platform(), "architecture": platform.machine(), "os": subprocess.check_output(["sw_vers"], text=True), "driver": subprocess.check_output(["/usr/bin/safaridriver", "--version"], text=True)}
try:
    created = request("POST", "/session", {"capabilities": {"alwaysMatch": {"browserName": "safari"}}})
    session = created["sessionId"]
    metadata["capabilities"] = created["capabilities"]
    command("POST", "/timeouts", {"script": 30000, "pageLoad": 60000, "implicit": 1000})
    command("POST", "/window/rect", {"width": 1000 if args.diagnose else 1440, "height": 700 if args.diagnose else 1000, "x": 0, "y": 0})
    if args.diagnose:
        subprocess.run(["swift", "-e", 'import AppKit; NSRunningApplication.runningApplications(withBundleIdentifier: "com.apple.Safari").first?.activate(options: [.activateIgnoringOtherApps])'], capture_output=True, text=True, timeout=30)
    for source, port in ((("beta17", 5317),) if args.diagnose else (("beta16", 5316), ("beta17", 5317))):
        command("POST", "/url", {"url": f"http://127.0.0.1:{port}/?s=claude&conversations&theme=warm-white"})
        wait_for("return !!document.querySelector('.sp-toggle')")
        js("localStorage.removeItem('side-open');localStorage.setItem('side-dock','right');localStorage.setItem('motion','always');")
        command("POST", "/refresh", {})
        wait_for("return !!document.querySelector('.sp-toggle')")
        time.sleep(2)
        metadata[source] = js("return {agent:navigator.userAgent,viewport:[innerWidth,innerHeight],screen:[screen.width,screen.height],href:location.href}")
        for dock in ("right", "bottom", "left"):
            if dock != "right":
                click(".sp-toggle")
                click(".sp-more")
                observe(f"{source}-{dock}-dock", lambda: click(f'.sp-menu-row[data-dock="{dock}"]'), True, dock)
                observe(f"{source}-{dock}-dock-close", lambda: click(".sp-toggle"), False, dock, 1)
            for cycle in range(2):
                observe(f"{source}-{dock}-open-{cycle}", lambda: click(".sp-toggle"), True, dock)
                observe(f"{source}-{dock}-close-{cycle}", lambda: click(".sp-toggle"), False, dock, 1)
        if source == "beta17":
            def reverse():
                click(".sp-toggle")
                time.sleep(0.08)
                click(".sp-toggle")
                time.sleep(0.08)
                click(".sp-toggle")
            observe("beta17-left-reverse", reverse, True, "left")
            observe("beta17-left-reverse-close", lambda: click(".sp-toggle"), False, "left", 1)
            js("const style=document.createElement('style');style.textContent='*, *::before, *::after { transition: none !important; animation: none !important; }';document.head.append(style);")
            observe("beta17-no-animation-open", lambda: click(".sp-toggle"), True, "left")
            observe("beta17-no-animation-close", lambda: click(".sp-toggle"), False, "left", 1)
    baseline = [item for item in results if item["label"].startswith("beta16-") and item["wantOpen"]]
    candidate = [item for item in results if item["label"].startswith("beta17-")]
    summary = {"baselineOpenCases": len(baseline), "baselineFailures": sum(not item["passed"] for item in baseline), "candidateCases": len(candidate), "candidateFailures": sum(not item["passed"] for item in candidate)}
    (out / "summary.json").write_text(json.dumps(summary, indent=2))
    print(json.dumps(summary), flush=True)
    if not args.diagnose and not summary["baselineFailures"]:
        raise AssertionError("The original beta.16 defect did not reproduce on this Safari; candidate results alone do not prove the fix.")
    if summary["candidateFailures"]:
        raise AssertionError("The candidate failed real Safari side panel checks.")
finally:
    if session:
        command("DELETE", "")
