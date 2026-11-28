# Windows Sandboxing (design note)

Antre has no OS-level sandbox backend on Windows yet. On a Windows desktop
without Docker, commands fall back to the [restricted host](sandbox.md#restricted-host-sandbox),
which isolates nothing and asks for approval on every command. This page
records the options studied for a native backend with the same policy as
[Seatbelt and bubblewrap](sandbox.md#os-sandbox-seatbelt--bubblewrap), and why
none ships yet.

## Target policy

Same rules as the macOS and Linux backends:

- system directories readable, not writable;
- the workspace readable (`ro`) or writable (`rw`), the sandbox home and temp
  writable, everything else in the user profile unreadable;
- Antre secrets and credential stores denied even inside allowed roots;
- network: none, egress proxy only, or open;
- the environment rebuilt from scratch;
- the whole process tree killed on timeout.

## Building blocks

### Job objects

A job object groups a process tree. It gives:

- kill-on-close (`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`), so a timeout kills
  every descendant;
- memory, CPU-rate and active-process limits (the Windows equivalent of our
  `ulimit` wrappers, and per job rather than per user);
- UI restrictions (no clipboard, no desktop switching, no global atoms).

Job objects provide no filesystem or network isolation. They are needed in
every design below, for lifetime and resource limits.

### Restricted tokens

`CreateRestrictedToken` derives a token from the user's token with SIDs
turned into deny-only, privileges removed, and a list of *restricting SIDs*.
An access check must then pass both for the normal SIDs and for the
restricting SIDs. Granting write access to the workspace means adding an
ACE for a restricting SID on the workspace directories.

- Strength: works on every supported Windows version, the process keeps
  running as the user (tools find their profile, PATH, etc.).
- Weakness: reads are hard to restrict. Most of the user profile grants read
  access to the user's SID; denying reads means adding deny ACEs to
  directories we do not own, or running with a low integrity level (which
  only blocks *writes* up the integrity ladder, not reads).
- Network is not covered: it needs Windows Filtering Platform rules or
  firewall rules per program, which require administrator rights to install.

OpenAI Codex ships a Windows sandbox built on restricted tokens and ACLs on
the writable roots, with an optional elevated setup step for stronger network
restrictions, which shows the approach is workable on real developer
machines.

### AppContainer

An AppContainer process runs with an AppContainer SID and a capability list.
By default it can read almost nothing in the user profile, cannot write
anywhere except its own container folder, and has no network unless granted
the `internetClient` capability.

- Strength: closest match to the target policy. Deny-by-default for both the
  filesystem and the network, enforced by the kernel, no admin rights needed
  for the sandbox itself.
- Granting the workspace means adding ACEs for the AppContainer SID on the
  workspace directories (and on the system directories a toolchain needs,
  when they are not already readable by `ALL APPLICATION PACKAGES`).
- Loopback is blocked for AppContainers: reaching the egress proxy on
  `127.0.0.1` needs a loopback exemption (`CheckNetIsolation LoopbackExempt`
  or `NetworkIsolationSetAppContainerConfig`), which applies to the whole
  container, not one port. The proxy-only network mode is therefore not
  achievable without extra filtering.
- Many developer tools (git, Python, Node installers) assume they can read
  the user profile and write to `%TEMP%`; each needs redirection through
  environment variables, as we already do for `HOME` and `TMPDIR`.

### Windows Sandbox and WSL 2

- *Windows Sandbox* (Hyper-V disposable VM) is strong but heavy, only on
  Pro/Enterprise editions, and has no programmatic API beyond `.wsb` files.
- *WSL 2* runs a Linux kernel: Antre could run commands with bubblewrap
  inside the WSL distribution. Strong isolation, but it needs WSL installed and
  the workspace crosses the 9P file-sharing boundary, which is slow.

## Rust ecosystem

- [`windows`](https://crates.io/crates/windows) and
  [`windows-sys`](https://crates.io/crates/windows-sys) (Microsoft) expose
  every API above: job objects, `CreateRestrictedToken`,
  `CreateAppContainerProfile`, `SECURITY_CAPABILITIES` in
  `STARTUPINFOEX`, ACL editing. They are raw bindings: process creation with
  attribute lists, ACL manipulation and handle inheritance must be written by
  hand.
- No mature, widely used crate wraps AppContainer or restricted-token
  sandboxing at the level `bwrap` or `sandbox-exec` give us. The small
  AppContainer crates found on crates.io have low adoption and little review,
  which is not enough for a security boundary.

## Decision

Not implemented for now. A correct backend means a few hundred lines of
unsafe Win32 code (attribute lists, ACLs, handle inheritance, job objects)
that must be tested on real Windows machines, and the proxy-only network mode
needs either admin-installed filtering rules or a loopback exemption that is
broader than the policy.

Recommended path when Windows desktops become a target:

1. Job object for every command on Windows, including the restricted host:
   kill-on-close and per-job limits are cheap and fix runaway process trees.
2. AppContainer backend with network `blocked` and `bypass` first; ACEs for
   the workspace, sandbox home and temp granted at session start and removed
   at cleanup.
3. Proxy-only network once a loopback exemption scoped to the proxy can be
   combined with a WFP filter, or through a per-session named-pipe bridge.

Until then, Docker Desktop (WSL 2 backend) is the recommended sandbox on
Windows.
