//! Launch at login with `SMAppService.mainApp` (macOS 13+), registered when the status says it
//! never was (`NotRegistered`). Turning it off in System Settings > General > Login Items leaves
//! a different status (it needs approval again), so the user's choice stands; a moved or
//! reinstalled bundle that lost its registration gets it back.

use objc2::rc::autoreleasepool;
use objc2_service_management::{SMAppService, SMAppServiceStatus};

/// Registers Ailfred as a login item if it never was. Blocking (a call to the service manager):
/// run it off the UI thread, and only from the installed bundle.
pub fn register_if_needed() {
    // A background thread has no autorelease pool of its own.
    autoreleasepool(|_| {
        // SAFETY: SMAppService is an XPC client, not bound to the main thread.
        let service = unsafe { SMAppService::mainAppService() };
        let status = unsafe { service.status() };
        if status != SMAppServiceStatus::NotRegistered {
            eprintln!("Ailfred login item: {}", name(status));
            return;
        }
        match unsafe { service.registerAndReturnError() } {
            Ok(()) => eprintln!("Ailfred login item: registered, now {}", name(unsafe { service.status() })),
            Err(e) => eprintln!("Ailfred login item: registering failed: {e:?}"),
        }
    });
}

/// Whether Ailfred opens at login now.
pub fn enabled() -> bool {
    // SAFETY: as above.
    autoreleasepool(|_| unsafe { SMAppService::mainAppService().status() } == SMAppServiceStatus::Enabled)
}

/// Turns launch at login on or off (the menu's checkmark). Returns the state after the change.
pub fn set(on: bool) -> bool {
    autoreleasepool(|_| {
        // SAFETY: as above.
        let service = unsafe { SMAppService::mainAppService() };
        let result = if on { unsafe { service.registerAndReturnError() } } else { unsafe { service.unregisterAndReturnError() } };
        if let Err(e) = result {
            eprintln!("Ailfred login item: turning it {} failed: {e:?}", if on { "on" } else { "off" });
        }
        let now = unsafe { service.status() };
        eprintln!("Ailfred login item: {}", name(now));
        now == SMAppServiceStatus::Enabled
    })
}

fn name(status: SMAppServiceStatus) -> &'static str {
    match status {
        SMAppServiceStatus::NotRegistered => "not registered",
        SMAppServiceStatus::Enabled => "enabled",
        SMAppServiceStatus::RequiresApproval => "off (needs approval in System Settings)",
        SMAppServiceStatus::NotFound => "not found",
        _ => "unknown",
    }
}
