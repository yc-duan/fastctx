use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::result::Result;

use tauri::Manager;
use tauri::ipc::Channel;
use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::*;
use windows::Win32::Foundation::{E_POINTER, RECT};
use windows::Win32::UI::Shell::SHCreateMemStream;
use windows::core::{HSTRING, Interface, PCWSTR, PWSTR};

use crate::{Bounds, Delivery, document};

struct Surface {
    controller: ICoreWebView2Controller,
    view: ICoreWebView2,
}

thread_local! {
    // COM controllers belong to Tauri's UI apartment. No Send wrapper is used.
    static SURFACES: RefCell<HashMap<String, Surface>> = RefCell::new(HashMap::new());
}

impl Drop for Surface {
    fn drop(&mut self) {
        if let Err(error) = unsafe { self.controller.Close() } { eprintln!("closing page view: {error}"); }
    }
}

fn text(get: impl FnOnce(*mut PWSTR) -> windows::core::Result<()>) -> windows::core::Result<String> {
    let mut value = PWSTR::null();
    get(&mut value)?;
    Ok(CoTaskMemPWSTR::from(value).to_string())
}

pub fn mount(window: tauri::WebviewWindow, id: String, html: String, bounds: Bounds, messages: Channel<Delivery>) -> Result<(), String> {
    if SURFACES.with(|all| all.borrow().contains_key(&id)) { return Err("page surface already exists".into()); }
    let hwnd = window.hwnd().map_err(|e| e.to_string())?;
    let directory = window.app_handle().path().app_cache_dir().map_err(|e| e.to_string())?.join("page-sandbox-webview");
    std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    let (send, receive) = std::sync::mpsc::channel();
    let options = CoreWebView2EnvironmentOptions::default();
    #[cfg(not(feature = "probe"))]
    let arguments = String::from(document::ARGUMENTS);
    // A probe build can try other arguments without being built again.
    #[cfg(feature = "probe")]
    let arguments = {
        let log = std::env::var("WORLD_PAGE_PROBE_NETLOG").map_err(|e| e.to_string())?;
        let chosen = std::env::var("WORLD_PAGE_PROBE_ARGS").unwrap_or_else(|_| document::ARGUMENTS.to_string());
        format!("{chosen} --remote-debugging-port=9432 --log-net-log=\"{log}\"")
    };
    unsafe { options.set_additional_browser_arguments(arguments); }
    let options: ICoreWebView2EnvironmentOptions = options.into();
    let directory = HSTRING::from(directory.as_os_str());
    CreateCoreWebView2EnvironmentCompletedHandler::wait_for_async_operation(
        Box::new(move |handler| unsafe {
            CreateCoreWebView2EnvironmentWithOptions(PCWSTR::null(), &directory, &options, &handler).map_err(Into::into)
        }),
        Box::new(move |status, environment| {
            status?;
            send.send(environment.ok_or_else(|| windows::core::Error::from(E_POINTER))).map_err(|_| windows::core::Error::from(E_POINTER))?;
            Ok(())
        }),
    ).map_err(|e| e.to_string())?;
    let environment = receive.recv().map_err(|e| e.to_string())?.map_err(|e| e.to_string())?;
    let (send, receive) = std::sync::mpsc::channel();
    let creator = environment.clone();
    CreateCoreWebView2ControllerCompletedHandler::wait_for_async_operation(
        Box::new(move |handler| unsafe { creator.CreateCoreWebView2Controller(hwnd, &handler).map_err(Into::into) }),
        Box::new(move |status, controller| {
            status?;
            send.send(controller.ok_or_else(|| windows::core::Error::from(E_POINTER))).map_err(|_| windows::core::Error::from(E_POINTER))?;
            Ok(())
        }),
    ).map_err(|e| e.to_string())?;
    let controller = receive.recv().map_err(|e| e.to_string())?.map_err(|e| e.to_string())?;
    let view = unsafe { controller.CoreWebView2() }.map_err(|e| e.to_string())?;
    let surface = Surface { controller, view };
    configure(&surface, &environment, html, messages).map_err(|e| e.to_string())?;
    let injector = surface.view.clone();
    AddScriptToExecuteOnDocumentCreatedCompletedHandler::wait_for_async_operation(
        Box::new(move |handler| unsafe {
            injector.AddScriptToExecuteOnDocumentCreated(&HSTRING::from(document::NAVIGATION), &handler).map_err(Into::into)
        }),
        Box::new(|status, _| status),
    ).map_err(|e| e.to_string())?;
    position(&surface, &bounds).map_err(|e| e.to_string())?;
    unsafe { surface.view.Navigate(&HSTRING::from(document::WRAPPER_URL)) }.map_err(|e| e.to_string())?;
    SURFACES.with(|all| all.borrow_mut().insert(id, surface));
    Ok(())
}

fn configure(surface: &Surface, environment: &ICoreWebView2Environment, html: String, messages: Channel<Delivery>) -> windows::core::Result<()> {
    let view = &surface.view;
    unsafe {
        let settings = view.Settings()?;
        settings.SetAreDefaultContextMenusEnabled(false)?;
        settings.SetAreDevToolsEnabled(cfg!(feature = "probe"))?;
        settings.SetAreDefaultScriptDialogsEnabled(false)?;
        settings.SetAreHostObjectsAllowed(false)?;
        settings.SetIsStatusBarEnabled(false)?;
        settings.SetIsBuiltInErrorPageEnabled(false)?;
        settings.cast::<ICoreWebView2Settings8>()?.SetIsReputationCheckingRequired(false)?;
        settings.cast::<ICoreWebView2Settings4>()?.SetIsGeneralAutofillEnabled(false)?;
        settings.cast::<ICoreWebView2Settings4>()?.SetIsPasswordAutosaveEnabled(false)?;

        let mut token = 0;
        let started = Cell::new(false);
        view.add_NavigationStarting(&NavigationStartingEventHandler::create(Box::new(move |_, args| {
            if let Some(args) = args {
                let url = text(|out| args.Uri(out))?;
                let allow = !started.replace(true) && url == document::WRAPPER_URL;
                args.SetCancel(!allow)?;
            }
            Ok(())
        })), &mut token)?;
        let frame_started = Cell::new(false);
        view.add_FrameNavigationStarting(&NavigationStartingEventHandler::create(Box::new(move |_, args| {
            if let Some(args) = args {
                let url = text(|out| args.Uri(out))?;
                let allow = !frame_started.replace(true) && url == document::CONTENT_URL;
                args.SetCancel(!allow)?;
            }
            Ok(())
        })), &mut token)?;
        view.add_NewWindowRequested(&NewWindowRequestedEventHandler::create(Box::new(|_, args| {
            if let Some(args) = args { args.SetHandled(true)?; }
            Ok(())
        })), &mut token)?;
        view.add_PermissionRequested(&PermissionRequestedEventHandler::create(Box::new(|_, args| {
            if let Some(args) = args { args.SetState(COREWEBVIEW2_PERMISSION_STATE_DENY)?; }
            Ok(())
        })), &mut token)?;
        view.cast::<ICoreWebView2_4>()?.add_DownloadStarting(&DownloadStartingEventHandler::create(Box::new(|_, args| {
            if let Some(args) = args { args.SetCancel(true)?; }
            Ok(())
        })), &mut token)?;
        view.cast::<ICoreWebView2_18>()?.add_LaunchingExternalUriScheme(&LaunchingExternalUriSchemeEventHandler::create(Box::new(|_, args| {
            if let Some(args) = args { args.SetCancel(true)?; }
            Ok(())
        })), &mut token)?;

        // Unlike the old filter, this includes cross-origin child frames.
        view.cast::<ICoreWebView2_22>()?.AddWebResourceRequestedFilterWithRequestSourceKinds(
            windows::core::w!("*"), COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL, COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_ALL,
        )?;
        let environment = environment.clone();
        let wrapper = document::wrapper(false);
        let served_host = Cell::new(false);
        let served_page = Cell::new(false);
        view.add_WebResourceRequested(&WebResourceRequestedEventHandler::create(Box::new(move |_, args| {
            let Some(args) = args else { return Ok(()); };
            let request = args.Request()?;
            let url = text(|out| request.Uri(out))?;
            let method = text(|out| request.Method(out))?;
            let mut context = COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL;
            args.ResourceContext(&mut context)?;
            let (body, policy, status) = if method == "GET" && url == document::CHECK_URL {
                ("canary reached native loader", "default-src 'none'", 200)
            } else if method == "GET" && context == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT && url == document::WRAPPER_URL && !served_host.replace(true) {
                (wrapper.as_str(), document::WRAPPER_POLICY, 200)
            } else if method == "GET" && context == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT && url == document::CONTENT_URL && !served_page.replace(true) {
                (html.as_str(), document::POLICY, 200)
            } else { ("", "default-src 'none'", 403) };
            let bytes = body.as_bytes();
            let stream = SHCreateMemStream(Some(bytes));
            let connections = if url == document::WRAPPER_URL { "(response-origin)" } else { "()" };
            let headers = HSTRING::from(format!("Content-Type: text/html; charset=utf-8\r\nContent-Security-Policy: {policy}\r\nConnection-Allowlist: {connections}\r\nAccess-Control-Allow-Origin: *\r\nX-DNS-Prefetch-Control: off\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\n"));
            let response = environment.CreateWebResourceResponse(stream.as_ref(), status, windows::core::w!("World page"), &headers)?;
            args.SetResponse(&response)
        })), &mut token)?;
        let delivery = messages.clone();
        view.add_WebMessageReceived(&WebMessageReceivedEventHandler::create(Box::new(move |_, args| {
            let Some(args) = args else { return Ok(()); };
            if text(|out| args.Source(out))? != document::WRAPPER_URL { return Ok(()); }
            let json = text(|out| args.WebMessageAsJson(out))?;
            match serde_json::from_str(&json) {
                Ok(value) => { if let Err(error) = delivery.send(Delivery::Message(value)) { eprintln!("delivering page message: {error}"); } }
                Err(error) => { let _ = delivery.send(Delivery::Failed(error.to_string())); }
            }
            Ok(())
        })), &mut token)?;
        view.add_ProcessFailed(&ProcessFailedEventHandler::create(Box::new(move |_, _| {
            let _ = messages.send(Delivery::Failed("the page's browser process stopped".into()));
            Ok(())
        })), &mut token)?;
    }
    Ok(())
}

fn position(surface: &Surface, bounds: &Bounds) -> windows::core::Result<()> {
    let coordinate = |value: f64| (value * bounds.scale).round() as i32;
    unsafe {
        surface.controller.SetBounds(RECT {
            left: coordinate(bounds.x), top: coordinate(bounds.y),
            right: coordinate(bounds.x + bounds.width), bottom: coordinate(bounds.y + bounds.height),
        })?;
        surface.controller.SetIsVisible(bounds.visible && bounds.width > 0.0 && bounds.height > 0.0)
    }
}

pub fn place(id: &str, bounds: &Bounds) -> Result<(), String> {
    SURFACES.with(|all| {
        let all = all.borrow();
        let surface = all.get(id).ok_or("page surface is closed")?;
        position(surface, bounds).map_err(|e| e.to_string())
    })
}

pub fn post(id: &str, message: &serde_json::Value) -> Result<(), String> {
    SURFACES.with(|all| {
        let all = all.borrow();
        let surface = all.get(id).ok_or("page surface is closed")?;
        unsafe { surface.view.PostWebMessageAsJson(&HSTRING::from(message.to_string())) }.map_err(|e| e.to_string())
    })
}

pub fn close(id: &str) -> Result<(), String> {
    SURFACES.with(|all| all.borrow_mut().remove(id));
    Ok(())
}

pub fn close_all() { SURFACES.with(|all| all.borrow_mut().clear()); }
