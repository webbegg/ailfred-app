//! Diagnosis probe for the popped-out chat's missing Dock icon: Ailfred's window layout (an
//! accessory run, a windowless host, a hidden second window), then the pop-out's calls, and
//! what macOS says the app is before and after. `PROBE=host` asks the host's handle for the
//! Dock icon (as Ailfred does), `PROBE=window` asks the window's own handle.

use std::time::Duration;

use rune::{App, AppSpec, RunOptions, WindowOptions, WindowSpec};

fn kind() -> String {
    let pid = std::process::id().to_string();
    let out = std::process::Command::new("lsappinfo").args(["info", "-only", "ApplicationType", &pid]).output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn main() {
    let variant = std::env::var("PROBE").unwrap_or_else(|_| "host".into());
    rune::run_windows_with(RunOptions { accessory: true }, move |_| {
        let mut app = App::new(AppSpec::new("Probe", (1.0, 1.0), || (), |_, _| rune::text(""))).windowless();
        let host = app.handle();
        let options = WindowOptions { visible: false, resizable: true, ..Default::default() };
        let (ui, window) = app.window(
            WindowSpec::from_app("chat", App::new(AppSpec::new("Chat", (400.0, 500.0), || (), |_, _| rune::text("chat"))).keep_running())
                .options(options),
        );
        let quit = host.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(2));
            println!("before: {}", kind());
            match variant.as_str() {
                "window" => ui.set_dock_icon(true),
                _ => host.set_dock_icon(true),
            }
            window.open();
            window.focus();
            for t in 1..=3 {
                std::thread::sleep(Duration::from_secs(1));
                println!("after {t}s ({variant}): {}", kind());
            }
            quit.quit();
        });
        vec![WindowSpec::from_app("host", app)]
    });
}
