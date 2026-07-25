use crate::config::types::RtkConfig;
use crate::extras::rtk::Rtk;

/// Writes a fake `rtk` binary that mimics the real contract:
/// `--version` exits 0; `rewrite` prints `rtk <cmd>` (exit 3) for commands
/// starting with an allowed prefix, otherwise exits 1 with no output.
fn fake_rtk(dir: &std::path::Path) -> String {
    let path = dir.join("rtk-fake");
    std::fs::write(
        &path,
        "#!/bin/sh\n\
         if [ \"$1\" = \"--version\" ]; then echo 'rtk 0.0.0-fake'; exit 0; fi\n\
         if [ \"$1\" = \"rewrite\" ]; then\n\
         case \"$2\" in git*|cargo*) echo \"rtk $2\"; exit 3;; *) exit 1;; esac\n\
         fi\n\
         exit 1\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    path.to_string_lossy().into_owned()
}

fn cfg(enabled: bool, path: Option<String>) -> RtkConfig {
    RtkConfig {
        enabled,
        path: path.map(Into::into),
    }
}

#[tokio::test]
async fn detect_disabled_returns_none() {
    assert!(Rtk::detect(None).await.is_none());
    assert!(Rtk::detect(Some(&cfg(false, None))).await.is_none());
}

#[tokio::test]
async fn detect_bogus_binary_returns_none() {
    let c = cfg(true, Some("/definitely/not/a/real/rtk".into()));
    assert!(Rtk::detect(Some(&c)).await.is_none());
}

#[cfg(unix)]
#[tokio::test]
async fn detect_and_rewrite_with_working_binary() {
    let dir = std::env::temp_dir().join(format!("zs_rtk_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = fake_rtk(&dir);

    let rtk = Rtk::detect(Some(&cfg(true, Some(path)))).await;
    assert!(rtk.is_some());
    let rtk = rtk.unwrap();

    // Supported command: rewritten (fake exits 3 — stdout is what matters).
    assert_eq!(
        rtk.rewrite("git status").await.as_deref(),
        Some("rtk git status")
    );
    // Unsupported command: no rewrite, caller keeps the original.
    assert_eq!(rtk.rewrite("echo hello").await, None);

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn resolve_rtk_filters_disabled() {
    use crate::config::Config;
    let mut c = Config::default();
    assert!(c.resolve_rtk().is_none());
    c.rtk = Some(cfg(false, None));
    assert!(c.resolve_rtk().is_none());
    c.rtk = Some(cfg(true, None));
    assert!(c.resolve_rtk().is_some());
}
