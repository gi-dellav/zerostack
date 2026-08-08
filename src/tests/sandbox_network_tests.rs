use std::path::PathBuf;

use clap::Parser;

use rig::tool::Tool;

use crate::agent::tools::bash::{BashTool, network_hint_for_exit};
use crate::cli::Cli;
use crate::config::Config;
use crate::sandbox::{Sandbox, network_hint};
use crate::tests::sandbox_support::{args_of, backend_sandbox, scratch_dir, triple_at};

const HINT: &str = "note: network is disabled by the sandbox (sandbox-network = false); ask the user whether it should be enabled";

fn dir(name: &str) -> PathBuf {
    scratch_dir("network", name)
}

/// A sandbox whose backend will run, so the bwrap policy (and with it the
/// network setting) actually applies.
fn running_sandbox(backend: &str, network: bool, name: &str) -> Sandbox {
    backend_sandbox(backend, Vec::new(), dir(name)).with_network(network)
}

/// The argv of one sandboxed command, with the scratch cache directory that
/// assembling it creates removed again.
fn wrap_args(sandbox: Sandbox, name: &str) -> Vec<String> {
    let args = args_of(&sandbox.wrap_command("echo hello").unwrap());
    let _ = std::fs::remove_dir_all(dir(name));
    args
}

fn index_of(args: &[String], flag: &str) -> Option<usize> {
    args.iter().position(|a| a == flag)
}

/// The warnings `build_sandbox` produces for a set of resolved settings. This
/// is the channel both entry points log once per session, so the conflict
/// warning is asserted where it is actually produced.
fn warnings_for(enabled: bool, network: bool) -> Vec<String> {
    crate::sandbox::build_sandbox(&crate::sandbox::SandboxSettings {
        enabled,
        required: false,
        backend: "bwrap",
        shell: "bash",
        expose: &[],
        network,
    })
    .warnings
}

fn warns_about_network(enabled: bool, network: bool) -> bool {
    warnings_for(enabled, network)
        .iter()
        .any(|w| w.contains("sandbox-network is set to false"))
}

// --- CLI parsing: bare flag means true ---

#[test]
fn test_bare_flag_parses_as_some_true() {
    let cli = Cli::try_parse_from(["zerostack", "--sandbox-network"]).unwrap();
    assert_eq!(cli.sandbox_network, Some(true));
}

// --- Resolver: CLI over config, default true ---

#[test]
fn test_resolve_sandbox_network_defaults_to_true() {
    let cli = Cli::default();
    let cfg = Config::default();
    assert!(
        cli.resolve_sandbox_network(&cfg),
        "the network stays open unless it is turned off explicitly"
    );
}

#[test]
fn test_resolve_sandbox_network_from_config() {
    let cli = Cli::default();
    let cfg = Config {
        sandbox: Some(true),
        sandbox_network: Some(false),
        ..Default::default()
    };
    assert!(!cli.resolve_sandbox_network(&cfg));
}

#[test]
fn test_resolve_sandbox_network_cli_disables_over_config() {
    let cli = Cli {
        sandbox_network: Some(false),
        ..Default::default()
    };
    let cfg = Config {
        sandbox: Some(true),
        sandbox_network: Some(true),
        ..Default::default()
    };
    assert!(!cli.resolve_sandbox_network(&cfg));
}

#[test]
fn test_resolve_sandbox_network_cli_enables_over_config() {
    // The direction a bare boolean flag could not express: the config turns
    // the network off and the command line wants it back for one run.
    let cli = Cli {
        sandbox_network: Some(true),
        ..Default::default()
    };
    let cfg = Config {
        sandbox: Some(true),
        sandbox_network: Some(false),
        ..Default::default()
    };
    assert!(cli.resolve_sandbox_network(&cfg));
}

#[test]
fn test_sandbox_network_never_enables_the_sandbox() {
    // `sandbox-network` is a modifier, not an enforcer: unlike
    // `sandbox-required` it must not switch the sandbox on by itself.
    let cli = Cli::default();
    let cfg = Config {
        sandbox_network: Some(false),
        ..Default::default()
    };
    assert!(
        !cli.resolve_sandbox(&cfg),
        "turning the network off must not imply sandbox = true"
    );
}

// --- The one-shot warning for a setting that cannot take effect ---

#[test]
fn test_network_off_with_the_sandbox_off_warns_once() {
    let warnings = warnings_for(false, false);
    let matched: Vec<&String> = warnings
        .iter()
        .filter(|w| w.contains("sandbox-network is set to false"))
        .collect();
    assert_eq!(
        matched.len(),
        1,
        "exactly one warning, naming the key: {warnings:?}"
    );
}

#[test]
fn test_no_warning_when_the_sandbox_is_on() {
    assert!(!warns_about_network(true, false));
}

#[test]
fn test_no_warning_when_the_network_stays_open() {
    assert!(!warns_about_network(false, true));
    assert!(!warns_about_network(true, true));
}

// --- Arg assembly: --unshare-net iff the network is resolved off ---

#[test]
fn test_default_emits_no_unshare_net() {
    let args = wrap_args(running_sandbox("bwrap", true, "default"), "default");
    assert!(
        index_of(&args, "--unshare-net").is_none(),
        "the default keeps the network, so the flag must be absent: {args:?}"
    );
}

#[test]
fn test_network_off_emits_unshare_net_in_the_unshare_block() {
    let args = wrap_args(running_sandbox("bwrap", false, "off"), "off");

    let cgroup = index_of(&args, "--unshare-cgroup")
        .unwrap_or_else(|| panic!("the existing unshare block should still be there: {args:?}"));
    let net = index_of(&args, "--unshare-net")
        .unwrap_or_else(|| panic!("expected --unshare-net when the network is off: {args:?}"));
    let die = index_of(&args, "--die-with-parent")
        .unwrap_or_else(|| panic!("--die-with-parent should still be emitted: {args:?}"));

    assert!(
        cgroup < net && net < die,
        "--unshare-net belongs in the unshare block, before --die-with-parent: {args:?}"
    );
}

#[test]
fn test_resolver_file_stays_bound_when_the_network_is_off() {
    // The /etc/resolv.conf bind is deliberately unconditional: keeping it out
    // of the branch is what keeps the rest of the assembly identical. The
    // resolver path comes through the test seam rather than from the host, so
    // this asserts on a real bind on every platform instead of skipping itself
    // wherever `/etc/resolv.conf` happens not to exist.
    let root = dir("resolv");
    std::fs::create_dir_all(&root).unwrap();
    let resolver = root.join("resolv.conf");
    std::fs::write(&resolver, "nameserver 127.0.0.53\n").unwrap();
    let expected = std::fs::canonicalize(&resolver).unwrap();

    let sandbox = running_sandbox("bwrap", false, "resolv").with_resolv_conf(resolver);
    let args = wrap_args(sandbox, "resolv");

    assert!(
        triple_at(
            &args,
            "--ro-bind-try",
            &expected.to_string_lossy(),
            "/etc/resolv.conf"
        )
        .is_some(),
        "the resolver bind must still be emitted with the network off: {args:?}"
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn test_network_off_changes_nothing_else_in_the_argv() {
    // The one-flag diff is the whole mechanism: the resolver bind, the masks,
    // the read-write binds and their order are identical either way, so
    // removing `--unshare-net` reproduces the default argv exactly.
    let with_network = wrap_args(running_sandbox("bwrap", true, "diff"), "diff");
    let without_network = wrap_args(running_sandbox("bwrap", false, "diff"), "diff");

    let stripped: Vec<String> = without_network
        .iter()
        .filter(|a| *a != "--unshare-net")
        .cloned()
        .collect();
    assert_eq!(
        stripped, with_network,
        "turning the network off must add exactly one flag and change nothing else"
    );
}

#[test]
fn test_zerobox_backend_ignores_the_network_setting() {
    // zerobox applies its own network policy; zerostack hands it the same
    // arguments either way.
    let args = wrap_args(running_sandbox("zerobox", false, "zerobox"), "zerobox");
    assert!(
        index_of(&args, "--unshare-net").is_none(),
        "--unshare-net is a bwrap flag and must never reach zerobox: {args:?}"
    );
}

#[test]
fn test_build_sandbox_wires_the_setting_through() {
    // The shared construction path is what both entry points use, so the
    // setting has to survive it and not just `Sandbox::with_network`.
    let setup = crate::sandbox::build_sandbox(&crate::sandbox::SandboxSettings {
        enabled: true,
        required: false,
        backend: "bwrap",
        shell: "bash",
        expose: &[],
        network: false,
    });
    let sandbox = setup
        .sandbox
        .with_backend_available(true)
        .with_cache_dir(dir("build"))
        .with_mask_roots(Vec::new());

    let args = wrap_args(sandbox, "build");
    assert!(
        index_of(&args, "--unshare-net").is_some(),
        "build_sandbox must forward sandbox-network to the sandbox it returns: {args:?}"
    );
}

// --- Hint: only when the network really is cut and the command failed ---

const DNS_FAILURE: &str = "curl: (6) Could not resolve host: example.com";

#[test]
fn test_hint_matches_the_no_network_failure_spellings() {
    for stderr in [
        DNS_FAILURE,
        "fatal: unable to access 'https://example.com/': Could not resolve host: example.com",
        "socket.gaierror: [Errno -3] Temporary failure in name resolution",
        "ping: connect: Network is unreachable",
        // A service on the *host's* loopback, which the sandbox's own private
        // loopback does not have: the common failure once the network is off.
        "curl: (7) Failed to connect to 127.0.0.1 port 5432: Connection refused",
        "bind: Cannot assign requested address",
    ] {
        assert_eq!(
            network_hint(stderr).as_deref(),
            Some(HINT),
            "expected a hint for: {stderr}"
        );
    }
}

#[test]
fn test_hint_matching_is_case_insensitive() {
    assert_eq!(
        network_hint("COULD NOT RESOLVE HOST: EXAMPLE.COM").as_deref(),
        Some(HINT)
    );
}

#[test]
fn test_module_resolution_failures_never_hint() {
    // The reason the pattern carries "host": bundlers and package managers say
    // "could not resolve" about imports and dependency trees, and blaming the
    // sandbox for a build error would send the model to the user with a
    // question about the wrong thing entirely.
    for stderr in [
        "X [ERROR] Could not resolve \"./missing-module\"",
        "npm ERR! code ERESOLVE\nnpm ERR! Could not resolve dependency:",
    ] {
        assert_eq!(
            network_hint(stderr),
            None,
            "a module resolution failure is not a network failure: {stderr}"
        );
    }
}

#[test]
fn test_unrelated_failure_never_hints() {
    assert_eq!(
        network_hint("cat: missing.txt: No such file or directory"),
        None,
        "the narrow pattern list is what keeps the sandbox from being blamed for every failure"
    );
}

#[test]
fn test_unlisted_wording_is_a_harmless_miss() {
    // Localized or unusual resolver messages are out of reach of a substring
    // list, as is a command that redirects stderr with `2>&1`; a missed hint
    // costs nothing, a wrong one costs the user a question.
    assert_eq!(network_hint("wget: unable to resolve host address"), None);
}

#[test]
fn test_exit_zero_never_hints() {
    // A command can print a resolver error inside a retry loop and still
    // succeed; the gate lives in the function the tool calls unconditionally.
    let sandbox = running_sandbox("bwrap", false, "exit-zero");
    assert_eq!(network_hint_for_exit(0, DNS_FAILURE, &sandbox), None);
    assert_eq!(
        network_hint_for_exit(6, DNS_FAILURE, &sandbox).as_deref(),
        Some(HINT)
    );
}

#[test]
fn test_open_network_never_hints() {
    // The same failure with the network on is a real resolver problem on the
    // host, and blaming the sandbox would send the model to the user with the
    // wrong question.
    let sandbox = running_sandbox("bwrap", true, "open");
    assert_eq!(network_hint_for_exit(6, DNS_FAILURE, &sandbox), None);
}

#[test]
fn test_unsandboxed_command_never_hints() {
    // Nothing unshared the network namespace, because nothing sandboxed the
    // command at all: the setting resolved to false, but no policy applied it.
    let sandbox = Sandbox::new(true, "bwrap")
        .with_backend_available(false)
        .with_network(false);
    assert_eq!(network_hint_for_exit(6, DNS_FAILURE, &sandbox), None);
}

#[test]
fn test_zerobox_command_never_hints() {
    // `--unshare-net` never reached zerobox, so whatever its own policy did is
    // not something this hint can explain.
    let sandbox = running_sandbox("zerobox", false, "zerobox-hint");
    assert_eq!(network_hint_for_exit(6, DNS_FAILURE, &sandbox), None);
}

// --- Tool description: told upfront, not discovered by failing a command ---

/// The base description, unchanged from before the key existed.
const BASE_DESCRIPTION: &str =
    "Execute a bash command in the current working directory. Returns stdout and stderr.";

/// The one sentence appended when the network really is unshared. Spelled out
/// here rather than imported so a silent reword of the tool definition, which
/// is prompt context the model reads and the user never sees, has to be a
/// deliberate edit in two places.
const NOTICE: &str = " This session's sandbox runs commands without network access, so the internet, the local network, and services listening on the host are unreachable; a server started and used within a single command still works on 127.0.0.1.";

fn bash_description(sandbox: Sandbox) -> String {
    BashTool::new(
        None,
        None,
        sandbox,
        None,
        #[cfg(feature = "rtk")]
        None,
    )
    .description()
}

#[test]
fn test_description_announces_an_unshared_network() {
    // Without this the model finds out only by running something that cannot
    // work, and its first guess at the cause is usually not the sandbox.
    let described = bash_description(running_sandbox("bwrap", false, "describe-off"));
    assert_eq!(
        described,
        format!("{BASE_DESCRIPTION}{NOTICE}"),
        "the notice is appended to the description, not substituted for it"
    );
}

#[test]
fn test_description_is_unchanged_when_the_network_is_open() {
    let described = bash_description(running_sandbox("bwrap", true, "describe-on"));
    assert_eq!(described, BASE_DESCRIPTION);
}

#[test]
fn test_description_stays_silent_when_nothing_unshares_the_network() {
    // The same gate as the hint: a session whose backend is missing, or whose
    // backend is zerobox, must not be told its commands are offline when they
    // are not. Telling the model the network is gone when it is not costs a
    // capability it actually has.
    let no_backend = Sandbox::new(true, "bwrap")
        .with_backend_available(false)
        .with_network(false);
    assert_eq!(bash_description(no_backend), BASE_DESCRIPTION);

    let zerobox = running_sandbox("zerobox", false, "describe-zerobox");
    assert_eq!(bash_description(zerobox), BASE_DESCRIPTION);

    let sandbox_off = Sandbox::new(false, "bwrap")
        .with_backend_available(true)
        .with_network(false);
    assert_eq!(bash_description(sandbox_off), BASE_DESCRIPTION);
}
