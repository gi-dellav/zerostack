use std::backtrace::Backtrace;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::Mutex;

use tracing_subscriber::EnvFilter;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::cli::Cli;
use crate::session::storage;

pub fn crash_log_dir() -> PathBuf {
    storage::data_dir().join("logs").join("crashes")
}

pub fn resolve_crash_log_path() -> PathBuf {
    let dir = crash_log_dir();
    let ts = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S");
    let pid = std::process::id();
    dir.join(format!("zerostack-crash-{ts}_{pid}.log"))
}

pub fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Some(path) = write_crash_report(info) {
            eprintln!("crash report written to {}", path.display());
        }
        default_hook(info);
    }));
}

fn write_crash_report(info: &std::panic::PanicHookInfo) -> Option<PathBuf> {
    let path = resolve_crash_log_path();
    if let Some(parent) = path.parent()
        && fs::create_dir_all(parent).is_err()
    {
        return None;
    }

    let mut content = String::from("zerostack crash report\n");
    content.push_str(&format!("time: {}\n", chrono::Local::now().to_rfc3339()));
    if let Some(version) = option_env!("CARGO_PKG_VERSION") {
        content.push_str(&format!("version: {version}\n"));
    }
    content.push('\n');

    if let Some(payload) = info.payload().downcast_ref::<&str>() {
        content.push_str(&format!("panic: {payload}\n"));
    } else if let Some(payload) = info.payload().downcast_ref::<String>() {
        content.push_str(&format!("panic: {payload}\n"));
    } else {
        content.push_str("panic: unknown\n");
    }

    if let Some(loc) = info.location() {
        content.push_str(&format!(
            "location: {}:{}:{}\n",
            loc.file(),
            loc.line(),
            loc.column()
        ));
    }

    content.push('\n');
    content.push_str(&format!("{:?}", Backtrace::capture()));

    fs::write(&path, content).ok().map(|_| path)
}

pub fn resolve_log_path(cli: &Cli) -> Option<PathBuf> {
    if let Some(ref path) = cli.log_file {
        return Some(path.clone());
    }
    if cli.verbose {
        let logs_dir = storage::data_dir().join("logs");
        fs::create_dir_all(&logs_dir).ok();
        let ts = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S");
        let pid = std::process::id();
        return Some(logs_dir.join(format!("zerostack-{ts}_{pid}.log")));
    }
    None
}

pub fn build_stderr_filter(cli: &Cli) -> EnvFilter {
    if let Some(ref lvl) = cli.log_level
        && let Ok(f) = EnvFilter::try_new(format!("{lvl},rig=off"))
    {
        return f;
    }
    if let Ok(f) = EnvFilter::try_from_default_env() {
        return f;
    }
    EnvFilter::new("warn,rig=off")
}

pub fn init(cli: &Cli) {
    let stderr_filter = build_stderr_filter(cli);
    let file_filter = EnvFilter::new("zerostack=trace,rig=off");

    let stderr_layer = tracing_subscriber::fmt::layer()
        .with_writer(io::stderr)
        .with_filter(stderr_filter);

    let registry = tracing_subscriber::registry().with(stderr_layer);

    let log_path = resolve_log_path(cli);
    if let Some(ref path) = log_path {
        match fs::File::create(path) {
            Ok(file) => {
                let file_layer = tracing_subscriber::fmt::layer()
                    .with_writer(Mutex::new(file))
                    .with_filter(file_filter);
                registry.with(file_layer).init();
                return;
            }
            Err(e) => {
                eprintln!(
                    "warning: could not create log file {}: {}",
                    path.display(),
                    e
                );
            }
        }
    }

    registry.init();
}
