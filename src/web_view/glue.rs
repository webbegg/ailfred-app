//! The web view's window in AppKit and WebKit (objc2): an `NSWindow` holding a `WKWebView`, the
//! page's message handler and the delegates that keep it from navigating or opening windows.
//! Main thread only. What it decides comes from `bridge`; this file only wires it, and is
//! checked by hand (it needs a window server).

use std::cell::Cell;

use block2::DynBlock;
use objc2::rc::Retained;
use objc2::runtime::{Bool, ProtocolObject, Sel};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSApplication, NSBackingStoreType, NSEvent, NSEventModifierFlags, NSResponder, NSWindow, NSWindowDelegate, NSWindowStyleMask, NSWorkspace};
use objc2_foundation::{NSNotification, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString, NSURL};
use objc2_web_kit::{
    WKNavigationAction, WKNavigationActionPolicy, WKNavigationDelegate, WKNavigationType, WKScriptMessage, WKScriptMessageHandler, WKUIDelegate,
    WKUserContentController, WKWebView, WKWebViewConfiguration, WKWindowFeatures,
};

use super::bridge::{self, Kind, Navigation};

/// The page's message handler's name: `window.webkit.messageHandlers.ailfred`.
const HANDLER: &str = "ailfred";
/// The window's size, in points, and the smallest it gets.
const SIZE: (f64, f64) = (520.0, 720.0);
const MIN_SIZE: (f64, f64) = (360.0, 420.0);

struct Ivars {
    /// What the page posted (its text), for the app.
    on_message: Box<dyn Fn(String)>,
    /// The window is closing.
    on_close: Box<dyn Fn()>,
    /// The page's first load went through: no other navigation does.
    loaded: Cell<bool>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and `Delegate` doesn't implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AilfredWebViewDelegate"]
    #[ivars = Ivars]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl WKScriptMessageHandler for Delegate {
        #[unsafe(method(userContentController:didReceiveScriptMessage:))]
        fn did_receive(&self, _: &WKUserContentController, message: &WKScriptMessage) {
            // Only strings: the page posts its messages as JSON text.
            let body = unsafe { message.body() };
            if let Ok(text) = body.downcast::<NSString>() {
                (self.ivars().on_message)(text.to_string());
            }
        }
    }

    unsafe impl WKNavigationDelegate for Delegate {
        #[unsafe(method(webView:decidePolicyForNavigationAction:decisionHandler:))]
        fn decide(&self, _: &WKWebView, action: &WKNavigationAction, decide: &DynBlock<dyn Fn(WKNavigationActionPolicy)>) {
            let url = unsafe { action.request() }.URL().and_then(|u| u.absoluteString()).map(|s| s.to_string()).unwrap_or_default();
            let kind = match unsafe { action.navigationType() } {
                WKNavigationType::LinkActivated => Kind::Link,
                WKNavigationType::Reload => Kind::Reload,
                _ => Kind::Other,
            };
            let main = unsafe { action.targetFrame() }.is_some_and(|f| unsafe { f.isMainFrame() });
            let navigation = if main || kind == Kind::Link { bridge::navigation(self.ivars().loaded.get(), kind, &url) } else { Navigation::Cancel };
            match navigation {
                Navigation::Allow => {
                    self.ivars().loaded.set(true);
                    decide.call((WKNavigationActionPolicy::Allow,));
                }
                Navigation::Cancel => {
                    eprintln!("Ailfred: the web view stayed on its page ({} chars of URL)", url.chars().count());
                    decide.call((WKNavigationActionPolicy::Cancel,));
                }
                Navigation::Open(url) => {
                    decide.call((WKNavigationActionPolicy::Cancel,));
                    open_url(&url);
                }
            }
        }
    }

    unsafe impl WKUIDelegate for Delegate {
        // No new windows: `window.open` and `target=_blank` get nothing.
        #[unsafe(method(webView:createWebViewWithConfiguration:forNavigationAction:windowFeatures:))]
        fn create_web_view(&self, _: &WKWebView, _: &WKWebViewConfiguration, _: &WKNavigationAction, _: &WKWindowFeatures) -> *mut WKWebView {
            std::ptr::null_mut()
        }
    }

    unsafe impl NSWindowDelegate for Delegate {
        #[unsafe(method(windowWillClose:))]
        fn window_will_close(&self, _: &NSNotification) {
            (self.ivars().on_close)();
        }
    }
);

define_class!(
    // SAFETY: NSWindow may be subclassed; `EditingWindow` adds no ivars and doesn't implement Drop.
    #[unsafe(super(NSWindow, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AilfredWebViewWindow"]
    struct EditingWindow;

    impl EditingWindow {
        // An accessory app has no Edit menu to send Cut, Copy, Paste and the rest: the window
        // sends them itself.
        #[unsafe(method(performKeyEquivalent:))]
        fn perform_key_equivalent(&self, event: &NSEvent) -> Bool {
            let flags = event.modifierFlags() & NSEventModifierFlags::DeviceIndependentFlagsMask;
            let command = flags.contains(NSEventModifierFlags::Command) && !flags.intersects(NSEventModifierFlags::Control | NSEventModifierFlags::Option);
            let key = event.charactersIgnoringModifiers().map(|k| k.to_string()).unwrap_or_default();
            if command && let Some(action) = super::edit_action(&key, flags.contains(NSEventModifierFlags::Shift)).and_then(selector) {
                let app = NSApplication::sharedApplication(self.mtm());
                if unsafe { app.sendAction_to_from(action, None, Some(self)) } {
                    return Bool::YES;
                }
            }
            unsafe { msg_send![super(self), performKeyEquivalent: event] }
        }
    }
);

fn selector(action: &str) -> Option<Sel> {
    Some(match action {
        "cut:" => sel!(cut:),
        "copy:" => sel!(copy:),
        "paste:" => sel!(paste:),
        "selectAll:" => sel!(selectAll:),
        "undo:" => sel!(undo:),
        "redo:" => sel!(redo:),
        "performClose:" => sel!(performClose:),
        _ => return None,
    })
}

/// The web view's window, open. Dropped, it lets go of everything: the window, the web view,
/// the handler (a `WKUserContentController` keeps its handler strongly) and the delegates.
pub struct Window {
    window: Retained<EditingWindow>,
    web_view: Retained<WKWebView>,
    controller: Retained<WKUserContentController>,
    /// Held here: the web view and the window keep their delegates weakly.
    #[allow(dead_code)]
    delegate: Retained<Delegate>,
}

impl Window {
    /// Opens the window in front, showing `html` (loaded with no base URL); `on_message` gets
    /// what the page posts, `on_close` is called as the window closes. None off the main thread.
    pub fn open(html: &str, on_message: impl Fn(String) + 'static, on_close: impl Fn() + 'static) -> Option<Window> {
        let mtm = MainThreadMarker::new()?;
        let delegate: Retained<Delegate> = {
            let this = Delegate::alloc(mtm).set_ivars(Ivars { on_message: Box::new(on_message), on_close: Box::new(on_close), loaded: Cell::new(false) });
            unsafe { msg_send![super(this), init] }
        };
        let controller = unsafe { WKUserContentController::new(mtm) };
        unsafe { controller.addScriptMessageHandler_name(ProtocolObject::from_ref(&*delegate), &NSString::from_str(HANDLER)) };
        let config = unsafe { WKWebViewConfiguration::new(mtm) };
        unsafe {
            config.setUserContentController(&controller);
            config.preferences().setJavaScriptCanOpenWindowsAutomatically(false);
        }
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(SIZE.0, SIZE.1));
        let web_view = unsafe { WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), frame, &config) };
        unsafe {
            web_view.setNavigationDelegate(Some(ProtocolObject::from_ref(&*delegate)));
            web_view.setUIDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        }
        // The Web Inspector (right click, Inspect Element) in a debug build only.
        if cfg!(debug_assertions) && web_view.respondsToSelector(sel!(setInspectable:)) {
            unsafe { web_view.setInspectable(true) };
        }
        let style = NSWindowStyleMask::Titled | NSWindowStyleMask::Closable | NSWindowStyleMask::Miniaturizable | NSWindowStyleMask::Resizable;
        let window: Retained<EditingWindow> =
            unsafe { msg_send![EditingWindow::alloc(mtm), initWithContentRect: frame, styleMask: style, backing: NSBackingStoreType::Buffered, defer: false] };
        // Owned here, not by AppKit: closing it mustn't free it under us.
        unsafe { window.setReleasedWhenClosed(false) };
        window.setTitle(&NSString::from_str("Ailfred"));
        window.setMinSize(NSSize::new(MIN_SIZE.0, MIN_SIZE.1));
        window.setContentView(Some(&web_view));
        window.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        window.center();
        unsafe { web_view.loadHTMLString_baseURL(&NSString::from_str(html), None) };
        let open = Window { window, web_view, controller, delegate };
        open.focus();
        Some(open)
    }

    /// Brings it to the front, the app active.
    pub fn focus(&self) {
        self.window.makeKeyAndOrderFront(None);
        #[allow(deprecated)]
        NSApplication::sharedApplication(self.window.mtm()).activateIgnoringOtherApps(true);
    }

    /// Runs `js` in the page; what it returns is dropped.
    pub fn eval(&self, js: &str) {
        unsafe { self.web_view.evaluateJavaScript_completionHandler(&NSString::from_str(js), None) };
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        unsafe {
            self.controller.removeScriptMessageHandlerForName(&NSString::from_str(HANDLER));
            self.web_view.setNavigationDelegate(None);
            self.web_view.setUIDelegate(None);
            self.web_view.stopLoading();
        }
        self.window.setDelegate(None);
        self.window.setContentView(None);
        // Already closed when the user closed it; closing again does nothing.
        self.window.close();
    }
}

/// Opens an `http(s)` link in the default browser; anything else is ignored.
pub fn open_url(url: &str) {
    if !bridge::openable(url) {
        return;
    }
    let Some(ns) = NSURL::URLWithString(&NSString::from_str(url)) else { return };
    // NSURL read the same scheme.
    let scheme = ns.scheme().map(|s| s.to_string().to_ascii_lowercase());
    if !matches!(scheme.as_deref(), Some("http" | "https")) {
        return;
    }
    if !NSWorkspace::sharedWorkspace().openURL(&ns) {
        eprintln!("Ailfred: the browser didn't open a link ({} chars)", url.chars().count());
    }
}
