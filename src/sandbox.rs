use std::collections::HashSet;
use std::process::{Output, Stdio};
use std::sync::{Arc, Mutex, OnceLock};

use tokio::io::AsyncReadExt;
use tokio::process::Command;

#[derive(Debug, Clone)]
pub struct Sandbox {
    enabled: bool,
    required: bool,
    backend: String,
    shell: String,
    /// `sandbox-network`: when false, the bwrap branch unshares the network
    /// namespace. Defaults to true, which is the historical behavior.
    network: bool,
    // Test seams: `backend_available` replaces the cached PATH probe for the
    // backend binary (and `backend_installed_now` the fresh one) so tests
    // behave the same on hosts with and without bwrap, `cache_dir` keeps the
    // bwrap arg builder away from the real user cache, `mask_roots`
    // replaces the built-in credential list with temp dirs so mask assertions
    // do not depend on the developer's home, and `resolv_conf` moves the
    // resolver file the sandbox binds off `/etc/resolv.conf`, which does not
    // exist on every host and so cannot be asserted on directly.
    backend_available: Option<bool>,
    backend_installed_now: Option<bool>,
    cache_dir: Option<std::path::PathBuf>,
    mask_roots: Option<Vec<std::path::PathBuf>>,
    resolv_conf: Option<std::path::PathBuf>,
    // Already-validated `sandbox-expose` paths (see `partition_expose`),
    // restored read-only on top of the masks.
    expose: Vec<std::path::PathBuf>,
    // `ssh_auth_sock` replaces the host `SSH_AUTH_SOCK` env read: `None` means
    // "use the real env var", `Some(None)` means "pretend it is unset",
    // `Some(Some(path))` pins the socket path a test masks.
    ssh_auth_sock: Option<Option<std::path::PathBuf>>,
    active_groups: Arc<Mutex<HashSet<u32>>>,
}

/// What `sandbox-network` does to the commands a session will actually run
/// (see `Sandbox::network_effect`). A resolved `false` is a request, not an
/// outcome: three things have to be true for it to reach a command, and
/// anything that reports the setting to a user has to say which one is
/// missing rather than printing the request back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NetworkEffect {
    /// `sandbox-network = true`: commands keep the host network, which is the
    /// default and the historical behavior.
    Open,
    /// Off and honored: every bash command runs in its own network
    /// namespace.
    Unshared,
    /// Off, but the sandbox is disabled, so nothing wraps the command.
    SandboxOff,
    /// Off, but the selected backend is not bwrap, whose flag this is.
    OtherBackend,
    /// Off, the sandbox is on and the backend is bwrap, but the bwrap binary
    /// is not installed: without `sandbox-required` the command falls back to
    /// running bare, with the network intact.
    BackendMissing,
}

/// Well-known credential stores that live directly under the home directory.
/// Each is covered by a tmpfs inside the bwrap sandbox, so sandboxed commands
/// read them as empty rather than as the user's keys and tokens.
const HOME_MASK_DIRS: [&str; 5] = [".ssh", ".aws", ".gnupg", ".kube", ".docker"];

/// The same, relative to the XDG config base rather than to the home
/// directory: `XDG_CONFIG_HOME` is forwarded into the sandbox by
/// `essential_env`, so a host that relocates its config base would otherwise
/// have these four masked in a place nothing reads and left readable (and
/// reachable) where the tools actually look.
const CONFIG_MASK_DIRS: [&str; 4] = ["gh", "gcloud", "op", "sops/age"];

/// The built-in mask list for a given home and XDG config base. Pure, so the
/// list can be asserted on without racing other tests over the process
/// environment. `xdg_config` is honored only when absolute, matching the XDG
/// spec (a relative `XDG_CONFIG_HOME` is invalid and must be ignored); note it
/// is deliberately *not* `dirs::config_dir()`, which resolves to
/// `~/Library/Application Support` on macOS, where none of these tools store
/// their credentials.
pub(crate) fn mask_roots_for(
    home: &std::path::Path,
    xdg_config: Option<&std::path::Path>,
) -> Vec<std::path::PathBuf> {
    let config_base = match xdg_config {
        Some(dir) if dir.is_absolute() => dir.to_path_buf(),
        _ => home.join(".config"),
    };
    HOME_MASK_DIRS
        .iter()
        .map(|rel| home.join(rel))
        .chain(CONFIG_MASK_DIRS.iter().map(|rel| config_base.join(rel)))
        .collect()
}

pub(crate) fn builtin_mask_roots() -> Vec<std::path::PathBuf> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let xdg_config = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from);
    mask_roots_for(&home, xdg_config.as_deref())
}

/// Splits raw `sandbox-expose` values into ones that restore read-only access
/// to a masked entry and ones to reject. A value is valid when, after `~` and
/// `$HOME` expansion against `home`, it equals a `mask_roots` entry or is a
/// subpath of one; `Path::starts_with` compares whole components, so `~/.ssh2`
/// is never mistaken for a subpath of `~/.ssh`. Values containing a `..`
/// component are rejected outright: `~/.ssh/..` passes the subpath test while
/// naming the whole home directory, which would re-bind everything the masks
/// hide. `home` is `None` on a host with no home directory, in which case
/// `~`/`$HOME` forms are left unexpanded (see `expand_tilde_with_home`) and so
/// never match a `mask_roots` entry. Pure: no warnings, no I/O.
pub(crate) fn partition_expose(
    raw: &[String],
    mask_roots: &[std::path::PathBuf],
    home: Option<&std::path::Path>,
) -> (Vec<std::path::PathBuf>, Vec<String>) {
    let mut valid = Vec::new();
    let mut rejected = Vec::new();
    for value in raw {
        let expanded = expand_expose(value, home);
        let escapes = expanded
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir));
        if !escapes && mask_roots.iter().any(|root| expanded.starts_with(root)) {
            valid.push(expanded);
        } else {
            rejected.push(value.clone());
        }
    }
    (valid, rejected)
}

/// `~`/`$HOME` expansion against an explicit home, so expose validation stays
/// pure while accepting exactly the spellings every other path-taking config
/// key accepts. `home` is forwarded to `expand_tilde_with_home` as-is, so the
/// no-home case (`None`) takes its documented "leave `~` forms unchanged"
/// path instead of one manufactured from an empty path.
fn expand_expose(value: &str, home: Option<&std::path::Path>) -> std::path::PathBuf {
    std::path::PathBuf::from(crate::fs::expand_tilde_with_home(value, home))
}

static BWRAP_AVAILABLE: OnceLock<bool> = OnceLock::new();

fn bwrap_exists() -> bool {
    *BWRAP_AVAILABLE.get_or_init(|| which_cmd("bwrap"))
}

static ZEROBOX_AVAILABLE: OnceLock<bool> = OnceLock::new();

fn zerobox_exists() -> bool {
    *ZEROBOX_AVAILABLE.get_or_init(|| which_cmd("zerobox"))
}

fn which_cmd(name: &str) -> bool {
    // Search PATH directly rather than shelling out to `which`, which may not
    // exist on minimal images (Alpine, distroless).
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| {
        let candidate = dir.join(name);
        candidate.is_file()
    })
}

pub(crate) struct ProcessGroupGuard {
    pid: Option<u32>,
    active_groups: Arc<Mutex<HashSet<u32>>>,
}

impl ProcessGroupGuard {
    pub(crate) fn new(pid: Option<u32>, active_groups: Arc<Mutex<HashSet<u32>>>) -> Self {
        if let Some(pid) = pid {
            active_groups
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(pid);
        }
        Self { pid, active_groups }
    }

    pub(crate) fn disarm(&mut self) {
        if let Some(pid) = self.pid.take() {
            self.active_groups
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&pid);
        }
    }
}

impl Drop for ProcessGroupGuard {
    fn drop(&mut self) {
        if let Some(pid) = self.pid.take() {
            self.active_groups
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&pid);
            kill_process_group(pid);
        }
    }
}

impl Sandbox {
    pub fn new(enabled: bool, backend: &str) -> Self {
        Sandbox {
            enabled,
            required: false,
            backend: backend.to_string(),
            shell: "bash".to_string(),
            network: true,
            backend_available: None,
            backend_installed_now: None,
            cache_dir: None,
            mask_roots: None,
            resolv_conf: None,
            expose: Vec::new(),
            ssh_auth_sock: None,
            active_groups: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// Returns true if the sandbox is enabled and the backend binary is
    /// actually available. When false, commands run unsandboxed (or are
    /// refused when the sandbox is required), and the UI should surface it.
    pub fn is_effectively_sandboxed(&self) -> bool {
        self.enabled && self.backend_available()
    }

    pub fn with_shell(mut self, shell: &str) -> Self {
        if !shell.is_empty() {
            self.shell = shell.to_string();
        }
        self
    }

    /// When required, bash commands are refused instead of running
    /// unsandboxed if the backend binary is missing.
    pub fn with_required(mut self, required: bool) -> Self {
        self.required = required;
        self
    }

    /// When false, each sandboxed command runs in a fresh network namespace
    /// with nothing in it but its own private loopback. A server the command
    /// starts and then talks to on `127.0.0.1` still works; everything on the
    /// host is gone, including services listening on the host's own loopback,
    /// and the namespace dies with the command, so a server backgrounded by
    /// one bash call is unreachable from the next. Bwrap backend only; zerobox
    /// applies its own network policy. `pub(crate)` rather than `pub`:
    /// construction is meant to go through `build_sandbox`, which pairs this
    /// with the sandbox-off conflict warning.
    pub(crate) fn with_network(mut self, network: bool) -> Self {
        self.network = network;
        self
    }

    /// Already-validated `sandbox-expose` paths (see `partition_expose`).
    /// Each is restored read-only on top of the masks via `--ro-bind-try`, so
    /// expose can never grant write access. `pub(crate)` rather than `pub`:
    /// construction is meant to go through `build_sandbox`, the one place
    /// that pairs this with `partition_expose`, so a caller outside the crate
    /// cannot hand it unvalidated paths and bind them read-only over the
    /// masks.
    pub(crate) fn with_expose(mut self, expose: Vec<std::path::PathBuf>) -> Self {
        self.expose = expose;
        self
    }

    #[cfg(test)]
    pub fn with_backend_available(mut self, available: bool) -> Self {
        self.backend_available = Some(available);
        self
    }

    /// Overrides the fresh probe only, so a test can model a backend that was
    /// installed after the cached probe ran.
    #[cfg(test)]
    pub fn with_backend_installed_now(mut self, installed: bool) -> Self {
        self.backend_installed_now = Some(installed);
        self
    }

    #[cfg(test)]
    pub fn with_cache_dir(mut self, dir: std::path::PathBuf) -> Self {
        self.cache_dir = Some(dir);
        self
    }

    #[cfg(test)]
    pub fn with_mask_roots(mut self, roots: Vec<std::path::PathBuf>) -> Self {
        self.mask_roots = Some(roots);
        self
    }

    /// Overrides the host resolver file the sandbox binds, so the bind can be
    /// asserted on from a host that has no `/etc/resolv.conf` (macOS in a
    /// minimal environment, some containers). Only the source path moves; the
    /// destination inside the sandbox stays `/etc/resolv.conf`, which is what
    /// production emits.
    #[cfg(test)]
    pub fn with_resolv_conf(mut self, path: std::path::PathBuf) -> Self {
        self.resolv_conf = Some(path);
        self
    }

    /// Overrides the host `SSH_AUTH_SOCK` read: `Some(path)` pins the socket
    /// path the agent-cutoff mask targets, `None` models a host with no
    /// ssh-agent running.
    #[cfg(test)]
    pub fn with_ssh_auth_sock(mut self, sock: Option<std::path::PathBuf>) -> Self {
        self.ssh_auth_sock = Some(sock);
        self
    }

    /// Credential directories this sandbox masks, narrowed to what exists on
    /// the host: bwrap creates every `--tmpfs` mountpoint, and creating one on
    /// the read-only `/` bind aborts the whole launch.
    pub(crate) fn masked_roots(&self) -> Vec<std::path::PathBuf> {
        if !self.bwrap_policy_active() {
            return Vec::new();
        }
        self.mask_roots
            .clone()
            .unwrap_or_else(builtin_mask_roots)
            .into_iter()
            .filter(|root| root.exists())
            .collect()
    }

    /// Whether the bwrap mount and namespace policy actually applies to a
    /// command: it needs the sandbox enabled, the bwrap backend, and a backend
    /// that will actually run. Credential masking and `sandbox-network` both
    /// hang off this, and the availability question is asked exactly the way
    /// `wrap_command` asks it, by sharing `backend_will_run` with it. Both
    /// directions of a mismatch hurt: a looser probe here would tell the model
    /// a path was masked (or the network cut) while the command ran bare and
    /// reached both fine, and a stricter one would leave a `sandbox-required`
    /// session running under bwrap with no masks emitted at all, which is the
    /// mount policy silently switching itself off for the user who asked for
    /// it hardest.
    fn bwrap_policy_active(&self) -> bool {
        self.backend_will_run() && self.backend_binary() == "bwrap"
    }

    /// Whether commands really do run without a network, which is the only
    /// state in which the network hint (and the tool-description notice built
    /// on the same question) is a true statement: `sandbox-network` is off
    /// *and* the bwrap policy that implements it is the one running.
    /// `pub(crate)` for the hint gate in `agent::tools::bash`.
    pub(crate) fn network_unshared(&self) -> bool {
        !self.network && self.bwrap_policy_active()
    }

    /// What `sandbox-network` actually does to this session's commands, and
    /// when it does nothing, which of the three preconditions is missing.
    /// `--print-config` renders this rather than the resolved boolean: the
    /// only affirmative answer is `bwrap_policy_active` itself, so a row that
    /// says the network is cut cannot disagree with the code that cuts it,
    /// and the remaining variants exist purely to explain a `false` that the
    /// session is not honoring.
    pub(crate) fn network_effect(&self) -> NetworkEffect {
        if self.network {
            return NetworkEffect::Open;
        }
        if self.bwrap_policy_active() {
            return NetworkEffect::Unshared;
        }
        if !self.enabled {
            return NetworkEffect::SandboxOff;
        }
        if self.backend_binary() != "bwrap" {
            return NetworkEffect::OtherBackend;
        }
        NetworkEffect::BackendMissing
    }

    /// The host's advertised ssh-agent socket, if any: the path
    /// `SSH_AUTH_SOCK` points to, which the bwrap branch masks with
    /// `/dev/null` so the agent stays unreachable even if a command
    /// reconstructs the variable by hand. Empty and stale values (a socket
    /// whose agent has since died, routine in long-lived tmux sessions) are
    /// dropped for the same reason `masked_roots` filters by `exists`: bwrap
    /// would have to create the bind destination on the read-only `/` bind,
    /// which fails and aborts every sandboxed command.
    fn ssh_auth_sock(&self) -> Option<std::path::PathBuf> {
        let advertised = match &self.ssh_auth_sock {
            Some(sock) => sock.clone(),
            None => std::env::var_os("SSH_AUTH_SOCK").map(std::path::PathBuf::from),
        };
        advertised.filter(|sock| !sock.as_os_str().is_empty() && sock.exists())
    }

    /// The mask root `cwd` sits under, if any. The working directory is bound
    /// after the masks, so it shadows that mask for its own subtree.
    pub(crate) fn shadowed_mask_root(&self, cwd: &std::path::Path) -> Option<std::path::PathBuf> {
        self.masked_roots()
            .into_iter()
            .find(|root| contained_in(cwd, root))
    }

    /// The once-per-session warning for a working directory that sits inside a
    /// masked credential directory, if it does. The directory is read from the
    /// process rather than taken as an argument because that is exactly what
    /// `wrap_command` binds: an ACP client's session directory is not the
    /// directory the sandbox mounts (the ACP server never chdirs), so warning
    /// about it would both fire spuriously and stay silent on the real case.
    pub(crate) fn shadowed_mask_warning(&self) -> Option<String> {
        let cwd = std::env::current_dir().ok()?;
        let root = self.shadowed_mask_root(&cwd)?;
        Some(format!(
            "sandbox: the working directory is inside {}, so the project bind partially shadows the mask on it",
            root.display()
        ))
    }

    /// Binary this sandbox probes for. Anything other than `zerobox` uses the
    /// bwrap backend.
    fn backend_binary(&self) -> &'static str {
        if self.backend == "zerobox" {
            "zerobox"
        } else {
            "bwrap"
        }
    }

    fn backend_available(&self) -> bool {
        if let Some(available) = self.backend_available {
            return available;
        }
        if self.backend == "zerobox" {
            zerobox_exists()
        } else {
            bwrap_exists()
        }
    }

    /// Same probe without the process-wide cache, so a backend installed while
    /// the session is running is picked up instead of staying "missing".
    fn backend_available_now(&self) -> bool {
        if let Some(available) = self.backend_installed_now.or(self.backend_available) {
            return available;
        }
        which_cmd(self.backend_binary())
    }

    /// Whether a command will actually be launched under the backend. This is
    /// the one predicate `wrap_command` branches on, and the one anything that
    /// describes what the sandbox does to a command (masking, and the hints
    /// built on it) has to ask: the sandbox is on, and either the cached PATH
    /// probe found the backend, or the sandbox is required and a fresh probe
    /// finds it. That second case is a backend installed after the
    /// process-wide probe ran, which `refusal_reason` already re-probes for,
    /// so a required sandbox launches on the fresh answer rather than the
    /// stale one.
    fn backend_will_run(&self) -> bool {
        self.enabled
            && (self.backend_available() || (self.required && self.backend_available_now()))
    }

    /// `Some(reason)` when the sandbox is required but its backend is missing,
    /// meaning bash commands must be refused rather than run unsandboxed.
    pub fn refusal_reason(&self) -> Option<String> {
        if !self.enabled || !self.required {
            return None;
        }
        if self.backend_available() || self.backend_available_now() {
            return None;
        }
        Some(format!(
            "sandbox backend '{}' not found; refusing to run because sandbox-required is set",
            self.backend
        ))
    }

    fn bare_command(&self, command: &str) -> Command {
        let mut cmd = Command::new(&self.shell);
        cmd.arg("-c").arg(command);
        configure_child_lifetime(&mut cmd);
        cmd
    }

    pub fn wrap_command(&self, command: &str) -> std::io::Result<Command> {
        if !self.enabled {
            return Ok(self.bare_command(command));
        }

        if let Some(reason) = self.refusal_reason() {
            return Err(std::io::Error::new(std::io::ErrorKind::NotFound, reason));
        }

        // Not refused and not launching means the sandbox is optional and its
        // backend is missing; `backend_will_run` is shared with
        // `bwrap_policy_active` so the two can never disagree about which
        // probe decides that.
        if !self.backend_will_run() {
            tracing::warn!(
                "sandbox: {} not found, running unsandboxed",
                self.backend_binary()
            );
            return Ok(self.bare_command(command));
        }

        let cwd = std::env::current_dir().unwrap_or_default();

        if self.backend == "zerobox" {
            let mut cmd = Command::new("zerobox");
            cmd.arg("--allow-write");
            cmd.arg(cwd.as_os_str());
            cmd.arg("--");
            cmd.arg(&self.shell);
            cmd.arg("-c");
            cmd.arg(command);
            configure_child_lifetime(&mut cmd);
            return Ok(cmd);
        }

        let mut cmd = Command::new("bwrap");
        cmd.arg("--clearenv");
        for (k, v) in essential_env() {
            cmd.arg("--setenv").arg(k).arg(v);
        }
        let resolv_conf = self
            .resolv_conf
            .clone()
            .unwrap_or_else(|| std::path::PathBuf::from("/etc/resolv.conf"));
        match std::fs::canonicalize(&resolv_conf) {
            Ok(target) => {
                cmd.arg("--ro-bind-try");
                cmd.arg(target);
                cmd.arg("/etc/resolv.conf");
            }
            Err(e) => {
                tracing::warn!(
                    "sandbox: no resolver file could be mounted: could not resolve {}: {}",
                    resolv_conf.display(),
                    e
                );
            }
        }
        // must bind /etc/resolv.conf before /.
        cmd.args(["--ro-bind", "/", "/"]);
        // Masks shadow the read-only root. The cwd bind below is allowed to
        // shadow a mask it falls *under*, so a project living inside a masked
        // directory stays usable; every other collision the read-write binds
        // cause is undone by the second mask layer further down.
        let masks = self.masked_roots();
        push_mask_layer(&mut cmd, &masks, &self.expose);
        cmd.arg("--bind");
        cmd.arg(cwd.as_os_str());
        cmd.arg(cwd.as_os_str());
        // Bind ~/.cache (or $XDG_CACHE_HOME) as writable after "/" bind
        let cache_dir = self.cache_dir.clone().or_else(dirs::cache_dir);
        if let Some(cache_dir) = &cache_dir {
            if let Err(e) = std::fs::create_dir_all(cache_dir) {
                tracing::warn!(
                    "sandbox: failed to create cache dir {}: {e}",
                    cache_dir.display()
                );
            }
            cmd.arg("--bind");
            cmd.arg(cache_dir.as_os_str());
            cmd.arg(cache_dir.as_os_str());
        }
        // Both binds above are read-write and land after the mask layer, so
        // every mask root *inside* one of them just came back, writable:
        // running zerostack from `$HOME` would otherwise hand the sandbox a
        // writable `~/.ssh`. Re-emit the mask layer for exactly those roots,
        // in the same mask-then-expose order, so what was exposed under them
        // stays visible read-only. Roots that *contain* the working directory
        // are deliberately left shadowed: the project bind keeps the last word
        // on its own subtree, which is what makes a project living inside a
        // masked directory usable.
        let reopened: Vec<std::path::PathBuf> = masks
            .iter()
            .filter(|root| {
                let root = root.as_path();
                std::iter::once(&cwd)
                    .chain(cache_dir.iter())
                    .any(|bind| contained_in(root, bind) && !same_dir(root, bind))
            })
            .cloned()
            .collect();
        let reopened_expose: Vec<std::path::PathBuf> = self
            .expose
            .iter()
            .filter(|path| reopened.iter().any(|root| path.starts_with(root)))
            .cloned()
            .collect();
        push_mask_layer(&mut cmd, &reopened, &reopened_expose);
        // The advertised ssh-agent socket goes last of all, after every bind
        // that could re-open it: an exposed `~/.gnupg` restores the gpg-agent
        // SSH socket, and a bind whose subtree contains the socket restores it
        // too. Masking it here means a command that reconstructs SSH_AUTH_SOCK
        // by hand still cannot reach the agent, with no configuration able to
        // undo it.
        if let Some(sock) = self.ssh_auth_sock() {
            cmd.arg("--ro-bind-try");
            cmd.arg("/dev/null");
            cmd.arg(sock.as_os_str());
        }
        cmd.args([
            "--ro-bind",
            "/sys",
            "/sys",
            "--proc",
            "/proc",
            "--dev",
            "/dev",
            "--tmpfs",
            "/tmp",
        ]);
        cmd.args([
            "--unshare-ipc",
            "--unshare-pid",
            "--unshare-uts",
            "--unshare-cgroup",
        ]);
        // `sandbox-network = false`. bwrap brings up a fresh loopback device
        // inside the new namespace, so this is not "no sockets": a command
        // that starts a server and then talks to it on 127.0.0.1 still works,
        // because both ends live in the same namespace. What goes away is the
        // host: the internet, the LAN, and anything already listening on the
        // *host's* loopback, plus the abstract Unix socket namespace, which
        // bwrap replaces along with the network one. The namespace lasts as
        // long as the command, so a server one bash call leaves running in the
        // background is unreachable from the next one. The /etc/resolv.conf
        // bind above stays unconditional: it costs nothing here, and keeping
        // it out of this branch is what keeps the rest of the argument
        // assembly identical between the two settings.
        if !self.network {
            cmd.arg("--unshare-net");
        }
        cmd.args(["--die-with-parent", &self.shell, "-c", command]);
        configure_child_lifetime(&mut cmd);
        Ok(cmd)
    }

    pub async fn output_command(&self, command: &str) -> std::io::Result<Output> {
        let mut cmd = self.wrap_command(command)?;
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = cmd.spawn()?;
        let (stdout_handle, stdout) = spawn_pipe_reader(child.stdout.take());
        let (stderr_handle, stderr) = spawn_pipe_reader(child.stderr.take());
        let mut guard = ProcessGroupGuard::new(child.id(), self.active_groups.clone());
        let status = child.wait().await?;

        if tokio::time::timeout(std::time::Duration::from_millis(100), async {
            join_reader(stdout_handle).await?;
            join_reader(stderr_handle).await
        })
        .await
        .is_err()
            && let Some(pid) = guard.pid
        {
            kill_process_group(pid);
        }
        let stdout = stdout.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let stderr = stderr.lock().unwrap_or_else(|e| e.into_inner()).clone();
        guard.disarm();
        Ok(Output {
            status,
            stdout,
            stderr,
        })
    }

    pub fn kill_active(&self) {
        let groups: Vec<u32> = self
            .active_groups
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain()
            .collect();
        for pid in groups {
            kill_process_group(pid);
        }
    }

    #[allow(dead_code)]
    pub fn active_group_count(&self) -> usize {
        self.active_groups
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }
}

/// Symlink-resolved containment: whether `inner` is `outer` or lives under it.
/// The mount policy turns on this question twice (which masks a read-write
/// bind re-opens, and whether the project bind shadows a mask), and comparing
/// the paths as written answers a different one. `cwd` comes from `getcwd(3)`
/// and is therefore fully resolved, while mask roots are built from `$HOME` as
/// the environment spells it, and bwrap resolves both at mount time. So
/// `~/.ssh -> ~/dotfiles/ssh` with the project at `~/dotfiles`, or a
/// `/home -> /data/home` layout with the project at `$HOME`, look unrelated
/// lexically while the kernel puts one inside the other, and the mask would be
/// left shadowed by a read-write bind: readable *and* writable, with no
/// warning.
///
/// A path that cannot be canonicalized (it went away after the `exists`
/// filter, or a parent denies traversal) counts as contained. Both callers use
/// containment to decide whether to mask *more*, so an unresolvable path errs
/// toward hiding credentials or warning about them; answering "not contained"
/// would be the silently permissive direction, which is the one that leaks.
fn contained_in(inner: &std::path::Path, outer: &std::path::Path) -> bool {
    if inner.starts_with(outer) {
        return true;
    }
    match (inner.canonicalize(), outer.canonicalize()) {
        (Ok(inner), Ok(outer)) => inner.starts_with(outer),
        _ => true,
    }
}

/// Whether two paths name the same directory once resolved, so a mask root a
/// read-write bind *is* can be told apart from one it *contains*: the former
/// is the project living inside a masked directory, where the bind keeps the
/// last word by design. Unresolvable paths are reported as different, which
/// leads to the same re-mask as `contained_in`'s fallback.
fn same_dir(a: &std::path::Path, b: &std::path::Path) -> bool {
    a == b || matches!((a.canonicalize(), b.canonicalize()), (Ok(a), Ok(b)) if a == b)
}

/// One layer of the credential mask: a tmpfs over each root, then the exposed
/// paths restored on top of them read-only. `--ro-bind-try` can never grant
/// write access, which is what turns expose's "only shrink, never widen" rule
/// from policy into mechanism, and the mask-before-expose order is what lets
/// expose re-open a hole at all. `wrap_command` emits this twice: once over
/// the read-only root, once over the read-write binds that follow it.
fn push_mask_layer(cmd: &mut Command, roots: &[std::path::PathBuf], expose: &[std::path::PathBuf]) {
    for root in roots {
        cmd.arg("--tmpfs");
        cmd.arg(root.as_os_str());
    }
    for path in expose {
        cmd.arg("--ro-bind-try");
        cmd.arg(path.as_os_str());
        cmd.arg(path.as_os_str());
    }
}

/// A session's sandbox plus the warnings building it produced. The warnings
/// are returned rather than logged so the rule they follow stays enforceable:
/// they are emitted once per session, from `init_features` and from ACP
/// `handle_new_session`, and never from a resolver or from per-prompt code.
pub(crate) struct SandboxSetup {
    pub sandbox: Sandbox,
    pub warnings: Vec<String>,
}

/// Already-resolved sandbox settings, as the CLI-over-config resolvers hand
/// them over.
pub(crate) struct SandboxSettings<'a> {
    pub enabled: bool,
    pub required: bool,
    pub backend: &'a str,
    pub shell: &'a str,
    /// Raw `sandbox-expose` values, unexpanded and unvalidated.
    pub expose: &'a [String],
    /// `sandbox-network`: false unshares the network namespace (bwrap only).
    pub network: bool,
}

/// Single construction path for the session sandbox, shared by every entry
/// point (startup and ACP), so expose validation and the warnings about it
/// cannot drift apart between them.
pub(crate) fn build_sandbox(settings: &SandboxSettings<'_>) -> SandboxSetup {
    let home = dirs::home_dir();
    let (expose, rejected) =
        partition_expose(settings.expose, &builtin_mask_roots(), home.as_deref());
    let sandbox = Sandbox::new(settings.enabled, settings.backend)
        .with_required(settings.required)
        .with_shell(settings.shell)
        .with_network(settings.network)
        .with_expose(expose);
    let mut warnings: Vec<String> = rejected
        .iter()
        .map(|value| {
            format!(
                "sandbox-expose value '{value}' is not a masked path or subpath of one, ignoring it"
            )
        })
        .collect();
    // `sandbox-network` is a modifier, not an enforcer: unlike
    // `sandbox-required` it never switches the sandbox on, so with the sandbox
    // off there is nothing to unshare and the setting silently does nothing.
    // The warning lives here rather than at the two call sites so the wording
    // exists once; both entry points already log everything in this list.
    if !settings.network && !settings.enabled {
        warnings.push(
            "sandbox-network is set to false but the sandbox is disabled, so commands still have network access"
                .to_string(),
        );
    }
    warnings.extend(sandbox.shadowed_mask_warning());
    SandboxSetup { sandbox, warnings }
}

/// One-line-per-root guidance for a failed sandboxed command: names every
/// masked root that `command` or `stderr` mentions, in either the `~/`
/// spelling or the absolute spelling under `home`. Pure and best-effort
/// (string containment, not path parsing), so it is safe to call on every
/// failure; its only caller, `bash::mask_hint_for_exit`, holds the exit-code
/// gate.
pub(crate) fn mask_hint(
    command: &str,
    stderr: &str,
    masked_roots: &[std::path::PathBuf],
    home: &std::path::Path,
) -> Option<String> {
    let mut lines = Vec::new();
    for root in masked_roots {
        let absolute = root.to_string_lossy();
        let tilde = root
            .strip_prefix(home)
            .ok()
            .map(|rel| format!("~/{}", rel.display()));
        let mentioned = command.contains(absolute.as_ref())
            || stderr.contains(absolute.as_ref())
            || tilde
                .as_deref()
                .is_some_and(|t| command.contains(t) || stderr.contains(t));
        if mentioned {
            let display = tilde.unwrap_or_else(|| absolute.into_owned());
            lines.push(format!(
                "note: {display} is masked by the sandbox; ask the user whether it should be exposed"
            ));
        }
    }
    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

/// The failure messages an unshared network produces, in the spellings the
/// common tools use. The first three are the name-resolution and raw-address
/// failures (curl and git print "Could not resolve host", Python and Go print
/// a "name resolution" failure, `ENETUNREACH` surfaces as "Network is
/// unreachable"); the last two are what reaching for a service on the *host's*
/// loopback looks like from inside a namespace that has its own, which is the
/// common case rather than an exotic one.
///
/// "could not resolve host" carries the "host" deliberately: bundlers say
/// `Could not resolve "./missing"` about imports and npm says the same about
/// dependency trees, and matching the shorter prefix would blame the sandbox
/// for build errors that have nothing to do with it.
const NETWORK_FAILURE_PATTERNS: [&str; 5] = [
    "could not resolve host",
    "name resolution",
    "network is unreachable",
    "connection refused",
    "cannot assign requested address",
];

/// Case-insensitive substring search that allocates nothing. The patterns are
/// pure ASCII, so a byte-window comparison answers the same question as
/// lowercasing both sides would, without copying stderr, which can be the
/// whole (untruncated) output of a failed build.
fn contains_ignore_ascii_case(haystack: &str, needle: &str) -> bool {
    let (haystack, needle) = (haystack.as_bytes(), needle.as_bytes());
    haystack.len() >= needle.len()
        && haystack
            .windows(needle.len())
            .any(|window| window.eq_ignore_ascii_case(needle))
}

/// The guidance line for a command that failed the way an unshared network
/// makes commands fail. Pure and deliberately narrow: the patterns are the
/// price of not blaming `sandbox-network` for every failing command, and a
/// hint that is missed costs nothing, while a hint that fires on an unrelated
/// failure sends the model to the user with the wrong question. The caller,
/// `bash::network_hint_for_exit`, holds the two state gates: the command
/// failed, and the network really is unshared.
///
/// Only stderr is matched. The command string is not, because a command that
/// merely mentions one of these phrases has not failed for that reason, and
/// stdout is not, because the phrases are common enough in ordinary output to
/// be a false-positive source. That makes the hint best-effort in both
/// directions: a command that redirects with `2>&1`, or one whose tooling
/// localizes its errors, produces nothing for the patterns to match and simply
/// gets no hint.
pub(crate) fn network_hint(stderr: &str) -> Option<String> {
    let matched = NETWORK_FAILURE_PATTERNS
        .iter()
        .any(|pattern| contains_ignore_ascii_case(stderr, pattern));
    matched.then(|| {
        "note: network is disabled by the sandbox (sandbox-network = false); ask the user whether it should be enabled"
            .to_string()
    })
}

fn spawn_pipe_reader(
    pipe: Option<impl tokio::io::AsyncRead + Send + Unpin + 'static>,
) -> (
    tokio::task::JoinHandle<std::io::Result<()>>,
    Arc<Mutex<Vec<u8>>>,
) {
    let output = Arc::new(Mutex::new(Vec::new()));
    let reader_output = output.clone();
    let handle = tokio::spawn(async move {
        if let Some(mut pipe) = pipe {
            let mut buf = [0; 8192];
            loop {
                let read = pipe.read(&mut buf).await?;
                if read == 0 {
                    break;
                }
                reader_output
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .extend_from_slice(&buf[..read]);
            }
        }
        Ok(())
    });
    (handle, output)
}

async fn join_reader(reader: tokio::task::JoinHandle<std::io::Result<()>>) -> std::io::Result<()> {
    reader
        .await
        .map_err(|e| std::io::Error::other(format!("pipe reader task failed: {e}")))?
}

pub(crate) fn configure_child_lifetime(cmd: &mut Command) {
    cmd.kill_on_drop(true);
    #[cfg(unix)]
    cmd.process_group(0);
}

pub(crate) fn kill_process_group(pid: u32) {
    #[cfg(unix)]
    {
        let group = format!("-{}", pid);
        let _ = std::process::Command::new("kill")
            .args(["-TERM", "--", &group])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        let _ = std::process::Command::new("kill")
            .args(["-KILL", "--", &group])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

/// The environment forwarded into the sandbox, read through `lookup` rather
/// than from the process. That is the seam: `std::env::set_var` is a data race
/// against every sibling test that reads the environment (this module reads it
/// for `SSH_AUTH_SOCK`, `XDG_CONFIG_HOME` and `PATH`), so the test that proves
/// the ssh-agent variables are dropped hands over an environment of its own
/// instead.
///
/// SSH_AUTH_SOCK and SSH_AGENT_PID are deliberately absent from the list:
/// forwarding them would keep a running ssh-agent reachable from inside the
/// sandbox, which the agent-socket mask closes even against a command that
/// reconstructs SSH_AUTH_SOCK by hand.
pub(crate) fn essential_env_from(
    lookup: impl Fn(&str) -> Option<String>,
) -> Vec<(&'static str, String)> {
    let preserve = [
        "PATH",
        "HOME",
        "USER",
        "LOGNAME",
        "SHELL",
        "TERM",
        "LANG",
        "LC_ALL",
        "SSH_ASKPASS",
        "GIT_ASKPASS",
        "DISPLAY",
        "WAYLAND_DISPLAY",
        "DBUS_SESSION_BUS_ADDRESS",
        "EDITOR",
        "VISUAL",
        "LD_LIBRARY_PATH",
        "CARGO_HOME",
        "RUSTUP_HOME",
        "GOPATH",
        "GOROOT",
        "VIRTUAL_ENV",
        "JAVA_HOME",
        "NODE_PATH",
        "TMPDIR",
        "XDG_RUNTIME_DIR",
        "XDG_CACHE_HOME",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_STATE_HOME",
        "COLORTERM",
        "NO_COLOR",
    ];
    let mut vars = Vec::with_capacity(preserve.len());
    for name in &preserve {
        if let Some(val) = lookup(name) {
            vars.push((*name, val));
        }
    }
    vars
}

fn essential_env() -> Vec<(&'static str, String)> {
    essential_env_from(|name| std::env::var(name).ok())
}
