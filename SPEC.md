# Crank Platform Specification

Status: Initial design for discussion; no implementation implied.

## 1. Product

Crank is a small, portable terminal application for discovering, inspecting,
and launching a curated collection of shell scripts on UNIX-like machines.

The primary experience is one command that downloads the correct prebuilt
binary and opens a clean, interactive TUI. Users do not need Rust, a package
manager, administrator access, or a permanent installation to launch Crank.

Linutil supplies the conceptual inspiration: metadata describes available
actions, a generic interface presents them, and scripts execute in an
interactive terminal. Crank is an independent design. It does not inherit
Linutil's catalog, branding, layout, implementation, or Linux-specific policy.

This specification concerns the platform and its distribution. Real utility
scripts, installation recipes, and system-management behavior come later.

## 2. Goals and scope

The initial platform must provide:

- One short curl command to download and launch a compatible release.
- A directly downloadable binary that also works without the bootstrap.
- A bundled catalog, script files, and supporting assets.
- A restrained TUI for browsing, searching, inspecting, and running actions.
- Interactive process execution with correct input, output, resizing, and
  cancellation behavior.
- Clear separation between catalog definitions, host capabilities, execution,
  and presentation.
- Explicit platform support backed by runnable release artifacts and tests.
- A development workflow for adding scripts without modifying the TUI.

The initial platform does not include package-manager abstractions, distro
setup, service management, an app store, remote catalog subscriptions,
background agents, telemetry, accounts, self-updating behavior, or a general
workflow engine. It does not implement rollback or infer what a script changes.

Multi-selection, batch execution, saved profiles, and unattended execution are
deferred. Version one runs one selected action at a time.

## 3. Portability contract

"Virtually any UNIX machine" is the portability goal, not a claim that one
executable runs on every kernel, architecture, or historical UNIX release.
Crank distributes a native binary for each supported target, selected by a
portable bootstrap script.

### 3.1 Release targets

The initial release matrix is a design target, not a statement of completed
support:

| Platform | Architectures | Distribution approach |
| --- | --- | --- |
| Linux | x86_64, aarch64 | Statically linked musl executables, verified on glibc and musl hosts |
| macOS | x86_64, aarch64 | Native executables with documented minimum OS versions |
| FreeBSD | x86_64 | Native executable with a documented supported release range |

FreeBSD aarch64, OpenBSD, NetBSD, illumos, and additional Linux architectures
are expansion candidates. Each becomes supported only after its binary,
terminal backend, bootstrap path, and execution behavior pass target-specific
validation. Compiler support alone is insufficient.

Release documentation must state minimum OS/kernel versions, CPU assumptions,
and remaining dynamic library requirements. Do not depend on the build
machine's CPU features or an undocumented recent libc.

### 3.2 Host requirements

Interactive use requires an accessible terminal with suitable terminal-control
support, PTY availability, and a writable temporary directory. Bootstrap use
also requires a POSIX shell, curl with HTTPS support, basic UNIX utilities, and
a supported SHA-256 verification command.

The binary does not require Bash, Python, Node.js, Git, sudo, a particular init
system, a graphical session, or a package manager to open its catalog. A
specific action may separately require an interpreter or command.

Support ordinary local terminals and SSH sessions with a terminal allocated.
Missing terminals, unsupported targets, insufficient permissions, and temporary
filesystems that prevent execution must produce actionable errors.

Host detection is read-only. Starting Crank never installs dependencies or
changes host configuration.

## 4. Download and launch

The intended public entry point has this shape:

```sh
curl -fsSL https://<release-host>/Crank.sh | sh
```

The URL is a placeholder. The domain, repository owner, and release hosting
provider remain to be chosen. Examples must not imply a live endpoint exists.

The POSIX bootstrap must:

1. Parse its options before doing work and detect OS and architecture using
   portable facilities such as `uname`.
2. Normalize aliases such as `amd64`/`x86_64` and `arm64`/`aarch64`.
3. Resolve the requested version, defaulting to the stable release, once.
4. Map that version and target to an explicitly published artifact. Never
   guess that a nearby target will work.
5. Create a private temporary directory and download the executable plus its
   checksum from the same immutable release.
6. Verify the executable before marking it executable or launching it.
7. Attach the application to the user's terminal, launch it, wait for it, clean
   up owned temporary files, and propagate its exit status.

Prefer a raw executable per target so the launch path does not require archive
extraction. The release metadata format must be readable without installing a
JSON processor or another runtime.

The piped shell's standard input contains bootstrap source, not user input.
Before starting an interactive child, the bootstrap must explicitly reconnect
its terminal streams through the controlling terminal, normally `/dev/tty`.
The child must not consume remaining bootstrap source as keyboard input.

The bootstrap must parse completely before starting the application, for
example by enclosing its implementation in a function and invoking it at the
end. Cleanup must remain owned by the bootstrap; blindly replacing it with
`exec` would discard its exit trap.

Support a pinned-version invocation in addition to the stable default:

```sh
curl -fsSL https://<release-host>/Crank.sh | sh -s -- --version <version>
```

Bootstrap options and binary options must be distinct; arguments after `--`
are passed to the binary without shell re-evaluation. A help request must not
download the application or require a terminal.

The default launch is ephemeral. It must not write to shell profiles, add
repositories, install packages, modify PATH, create persistent caches, or
request elevated privileges. Users may separately download the binary and
place it on PATH. An installation mode is outside the initial scope.

Downloads must use HTTPS and fail on HTTP errors, incomplete transfers, and
checksum mismatch. Checksums detect corruption and mismatched artifacts;
publisher trust still comes from the bootstrap and release delivery channel.
Release signing is a separate distribution decision, not something a checksum
alone provides.

## 5. Platform architecture

Use Rust for the application and a small POSIX shell bootstrap for delivery.
Keep the following boundaries explicit. Modules may precede separate crates
where that keeps the implementation small.

| Boundary | Responsibility |
| --- | --- |
| Core | Catalog schema, stable action IDs, requirements, validation, and action lifecycle types |
| Runtime | Embedded assets, extraction lifetime, host probing, interpreter resolution, process execution, and runtime events |
| TUI | Navigation, search, details, confirmation, terminal rendering, and input routing |
| Tooling | Catalog validation, harmless fixtures, release packaging, and generated documentation |
| Catalog | Declarative metadata, script files, and assets; no UI code |

Core must not depend on the TUI or spawn processes. Runtime must not know widget
types or keybindings. The TUI must not branch on individual action IDs, script
filenames, distributions, or package managers.

The runtime reports events such as `Started`, `Output`, `Exited`, and `Failed`.
The TUI translates events into application state and rendering. Process
ownership must remain independent of widget drawing.

Prefer a synchronous application loop with a small number of worker threads
and bounded communication. An async runtime is not required by this scope.

### 5.1 Dependency direction

```text
TUI --------> Core
 |              ^
 +--> Runtime --+

Tooling -----> Core
             Runtime, only when validating bundled assets
```

Start with Ratatui and Crossterm for the interface, serde and TOML for metadata,
clap for CLI parsing, and an embedded-directory plus temporary-directory
mechanism for bundled assets. Evaluate portable-pty with a compatible terminal
parser and Ratatui terminal widget across the release matrix before committing
to that execution stack.

Prefer existing terminal emulation components to writing an escape-sequence
parser. Do not inherit forks, dependency versions, or presentation dependencies
from Linutil without a concrete need. Images, random tips, and syntax
highlighting are unnecessary for the first release.

## 6. Catalog and action model

The catalog is a versioned document bundled with the executable. A release
contains a consistent snapshot of metadata, scripts, and assets. Once
downloaded, browsing and launching bundled demonstration actions works offline.

Define these distinct concepts:

- `ActionId`: unique, stable, machine-readable identifier.
- `ActionDefinition`: label, description, category reference, script path,
  interpreter definition, and optional host requirements.
- `Category`: navigation metadata that groups action IDs; never executable.
- `Availability`: available or unavailable, with an explanation.
- `ResolvedCommand`: interpreter, argument vector, working directory, and
  ownership of any required extracted assets.
- `ExecutionResult`: action ID, success/failure/cancellation, exit code or
  signal when available, and a contextual launch error when applicable.

Display names are not identifiers. Renaming or moving an action must not change
its identity. Ordering must be deterministic.

Use one file-based script execution form initially. Inline shell strings,
parameter templates, expression languages, and dynamically fetched code are
outside the first schema.

Interpreter metadata is explicit and authoritative, including any interpreter
arguments. Default to `sh` resolved on PATH. If a script has a shebang, catalog
validation must check that it is consistent with the declared interpreter;
support for complex shebang forms is unnecessary for the initial contract.

Validate duplicate IDs, unknown fields, schema versions, missing scripts,
invalid category references, and paths that escape the catalog root. Categories
must not contain executable fields. A bad catalog yields contextual errors,
not a panic or silently missing content.

Initial requirements are limited to OS, architecture, interpreter availability,
and required executables. Preserve the reason for unavailability. Show
compatible actions by default and provide a way to inspect unavailable actions
and their reasons. Never offer a generic bypass that forces an incompatible
action to run.

Provide a developer option to load a local catalog directory through the same
schema and validation path. Clearly identify local catalog mode in the UI.
Automatic remote catalog loading is deferred.

## 7. Script-launching contract

The runtime treats a script as an executable payload with declared requirements.
It does not understand package installation, distro configuration, or the
script's internal steps.

- Keep the bundled directory structure intact when extracting assets.
- Own the extracted directory until every process using it has terminated.
- Set the child working directory to the script's parent directory so relative
  imports and sibling assets work predictably.
- Launch the interpreter with an argument vector containing the script path.
  Never flatten arguments or working directories into interpolated shell text.
- Preserve the caller's ordinary environment, including PATH, HOME, and locale.
  Apply only the terminal settings needed for the child session, matching the
  emulator's actual capabilities.
- Recheck action availability immediately before launching.
- Run as the invoking user. Crank does not escalate the entire application.
- Capture exit status and launch errors separately. Cancellation is a distinct
  outcome, not success.

Only one child action may be active. Future batching must introduce explicit
per-action results and failure policy; it must not concatenate script commands
into a shared shell program.

Scripts are trusted executable code, not sandboxed plugins. This specification
does not prescribe their internal logic or claim to contain their effects.

## 8. TUI experience

The interface should feel like a compact tool picker. Its default screen has:

- A small application title and version.
- A category list and an action list, with a single-list layout on narrow
  terminals.
- Search and a concise description of the highlighted action.
- A short, context-sensitive keyboard hint line.

Do not add a dashboard, image logo, machine-statistics panel, random tips,
promotional content, persistent decorative panels, or multiple theme systems.

Required interactions:

- Arrow keys navigate; Enter opens a category or requests an action run.
- `/` focuses search across action names and descriptions.
- A visible shortcut opens a read-only script preview.
- Escape moves back or closes an inspection view.
- A concise help view lists the available keys.
- Mouse interaction may be added, but every operation works from the keyboard.

Selection must never launch an action as a side effect of navigation. Before
execution, show the action name and description with explicit Run and Cancel
choices; Cancel is the default. Do not hard-code exceptions for individual
actions. Preview is source inspection, not a prediction of system changes.

Use one readable color palette, ordinary terminal fonts, and ASCII-compatible
fallbacks. Do not require image protocols, Nerd Fonts, or truecolor. Respect
color-disable preferences, and never communicate status only through color.

Target useful navigation at 80x24. Adapt layout as the terminal shrinks. Below
the usable minimum, display a resize message while preserving state; do not
exit or require a size-bypass flag.

### 8.1 Running an action

Switch to a dedicated execution view with the action name, terminal viewport,
and a small status line. Allocate a real PTY so prompts, password input,
progress output, and terminal applications behave interactively.

Forward keyboard input and resize events. Ctrl-C should reach the child's
foreground terminal process group as an interrupt. Provide an explicit way to
request termination if the child ignores interruption, followed by a bounded
escalation path for forced termination of the managed session.

Preserve application control without silently stealing ordinary script input.
Document any reserved key chord. Do not close an active session without making
its cancellation explicit.

Update a persistent terminal parser incrementally. Bound scrollback and memory
use; do not retain and reparse an unlimited output transcript on each redraw.
Persistent log storage is outside the first release.

After exit, leave the final output visible with success, failure, or cancelled
status. Returning to the catalog preserves navigation and search state.

## 9. Lifecycle and errors

Normal exit, launch failure, cancellation, and recoverable application errors
must restore terminal mode, cursor visibility, and the original screen. Use
explicit terminal-session ownership and appropriate panic/signal handling.
Uncatchable termination cannot guarantee cleanup and must not be represented
as doing so.

Quitting an active action must terminate and reap managed child processes
before releasing their assets. Do not leave detached background processes as
an accidental consequence of dropping an execution view. Deliberately
daemonizing scripts are outside the initial script contract.

Library boundaries return contextual errors. A failed action must not crash
the application or make the catalog unusable. An empty or fully unavailable
catalog produces an explanatory view.

The binary's exit status describes the application session: zero for normal
user exit, nonzero for startup or application failure. Individual script exit
statuses appear in their execution results. A future direct-run CLI must
define its own script-status propagation contract.

`--help` and `--version` work without a terminal. Interactive startup without a
usable terminal fails clearly. Local binary use performs no update checks or
network access on its own.

## 10. Validation and release gates

Build the platform using harmless fixture scripts: print output, prompt for
input, fail with a known exit code, wait for interruption, display terminal
colors, and access a sibling asset. These fixtures exercise the launcher
contract; they are not the future utility catalog.

Automated validation must cover:

- Catalog parsing, stable IDs, invalid metadata, missing assets, and
  unavailable-action explanations.
- Script and temporary-directory paths containing spaces and shell
  metacharacters; arguments must arrive unchanged.
- PTY input/output, terminal resizing, completion, cancellation, and child
  cleanup on each supported OS family.
- Bounded output handling under sustained output.
- TUI navigation, searching, confirmation, empty states, and narrow layouts.
- Terminal restoration after application and process errors.
- Bootstrap target mapping, pinned releases, failed downloads, checksum
  mismatch, quoted argument forwarding, and cleanup.
- The actual `curl | sh` path under a controlling terminal, including SSH use.

Run Rust formatting, unit/integration tests, and Clippy with warnings treated
as errors. Check the POSIX bootstrap with ShellCheck and a portability check.
Test platform fixtures in controlled CI environments; no real system-changing
utility is needed for platform acceptance.

Every advertised target requires a real release artifact and execution tests
on that OS, using native runners or suitable virtual machines. A successful
cross-compilation is not sufficient. Exercise Linux artifacts on both glibc
and musl hosts, and publish the tested OS baselines with each release.

## 11. Initial completion criteria

The platform is ready for work on real scripts when a user can:

1. Run the public bootstrap command on each advertised target without root or
   a preinstalled development toolchain.
2. Browse and search the bundled demonstration catalog in a clean TUI.
3. Inspect a script and confirm its execution.
4. Interact with it through the PTY, resize the terminal, and interrupt it.
5. See an accurate result and return to the catalog.
6. Exit with the terminal restored and owned temporary files cleaned up.
7. Download the same binary directly and use the bundled catalog offline.

Implementation should proceed in that order of responsibility: catalog and
validation, independent runtime, minimal TUI, bootstrap and release matrix,
then target-specific verification. Script functionality remains a later phase.

## 12. Decisions reserved for the next discussion

- Final public name, repository location, release host, and bootstrap URL.
- Minimum OS versions and whether all three initial OS families gate the
  first public release or are introduced in documented stages.
- Final metadata syntax and local catalog authoring workflow.
- The terminal parser/widget combination after cross-platform evaluation.
- Release-signing policy and optional permanent installation UX.

These open decisions do not authorize implementing system-management scripts
or expanding the initial feature set.

## References

- Rust target availability and support tiers:
  <https://doc.rust-lang.org/rustc/platform-support.html>.
  Crank support requires its own runtime validation beyond compiler support.
- Candidate PTY interface:
  <https://docs.rs/portable-pty/0.9.0/portable_pty/>.
  Its API separates PTY allocation, process launch, input, output, and resizing.
