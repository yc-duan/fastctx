import argparse, base64, json, pathlib, subprocess, time, urllib.request, urllib.parse
p=argparse.ArgumentParser();p.add_argument('--out',required=True);out=pathlib.Path(p.parse_args().out);out.mkdir(parents=True,exist_ok=True)
base='http://127.0.0.1:4444';session=None;records=[]
def req(method,path,data=None):
 r=urllib.request.Request(base+path,data=None if data is None else json.dumps(data).encode(),method=method,headers={'Content-Type':'application/json'})
 with urllib.request.urlopen(r,timeout=60) as x:return json.load(x).get('value')
def call(method,path,data=None):return req(method,'/session/'+session+path,data)
def js(text):return call('POST','/execute/sync',{'script':text,'args':[]})
def capture(name):
 time.sleep(2)
 state=js("return {url:location.href,title:document.title,fonts:document.fonts.status,text:document.body.textContent.length,styles:[...document.querySelectorAll('body,.shell,.sidebar,.sp-panel,.sp-sheet,.sh-page,.page-head')].map(e=>{const s=getComputedStyle(e);return {class:e.className,opacity:s.opacity,visibility:s.visibility,display:s.display,font:s.fontSize,color:s.color,background:s.backgroundColor,mask:s.maskImage,filter:s.filter,contain:s.contain,contentVisibility:s.contentVisibility,transform:s.transform}})}")
 (out/(name+'.png')).write_bytes(base64.b64decode(call('GET','/screenshot')))
 native=subprocess.run(['screencapture','-x',str(out/('native-'+name+'.png'))],capture_output=True,text=True,timeout=10)
 records.append({'name':name,'state':state,'native':native.returncode});(out/'paint.json').write_text(json.dumps(records,indent=2));print(name,state,flush=True)
def app():
 call('POST','/url',{'url':'http://127.0.0.1:5317/?s=claude&conversations&theme=warm-white'})
 for _ in range(120):
  if js("return !!document.querySelector('.sp-toggle')"):break
  time.sleep(.2)
 time.sleep(2)
 js("document.querySelector('.sp-toggle').click();")
try:
 session=req('POST','/session',{'capabilities':{'alwaysMatch':{'browserName':'safari'}}})['sessionId']
 call('POST','/window/rect',{'width':1000,'height':700,'x':0,'y':0})
 subprocess.run(['swift','-e','import AppKit; NSRunningApplication.runningApplications(withBundleIdentifier: "com.apple.Safari").first?.activate(options: [.activateIgnoringOtherApps])'],capture_output=True,timeout=30)
 plain='<html><body style="margin:0;background:#ffcc00;color:#000;font:32px sans-serif"><h1>Simple paint control</h1><p>No application CSS or scripts.</p><div style="background:#00aaff;height:150px">Blue rectangle</div></body></html>'
 call('POST','/url',{'url':'data:text/html;charset=utf-8,'+urllib.parse.quote(plain)});capture('simple')
 app();capture('app')
 js("for(const sheet of document.styleSheets)sheet.disabled=true;");capture('no-styles')
 tests={
  'no-animations':'*,*::before,*::after{animation:none!important;transition:none!important}',
  'opaque':'*,*::before,*::after{opacity:1!important}',
  'no-effects':'*,*::before,*::after{filter:none!important;backdrop-filter:none!important;-webkit-backdrop-filter:none!important;mask:none!important;-webkit-mask:none!important;clip-path:none!important;transform:none!important;content-visibility:visible!important;contain:none!important;mix-blend-mode:normal!important}',
 }
 for name,css in tests.items():
  app();js('const style=document.createElement("style");style.textContent='+json.dumps(css)+';document.head.append(style)');capture(name)
 app();js("for(const sheet of document.styleSheets){try{for(let i=sheet.cssRules.length-1;i>=0;i--)if(sheet.cssRules[i].cssText.startsWith('@property'))sheet.deleteRule(i);}catch{}}")
 capture('no-registrations')
finally:
 if session:call('DELETE','')
