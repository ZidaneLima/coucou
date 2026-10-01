# Coucou — guide for opencode in this fork

Coucou draws Mochi, a character that lives in a panel at the top of the screen,
shows your Claude Code and opencode sessions, and lets you approve, answer and
chat from there.

This fork (`ZidaneLima/coucou`, branch `arch-linux`) targets **Arch Linux and
Omarchy on Hyprland**. `upstream` is `Louis-CFM/coucou`.

`CLAUDE.md` is upstream's guide and describes only the macOS half. This file
replaces it for opencode sessions, so everything needed is here.

## Two halves, one product

- `NotchBuddy/` — the original macOS app, Swift 6 + SwiftUI + AppKit, drawn in
  code. Not what this fork works on. `cd NotchBuddy && xcodegen && xcodebuild
  -scheme NotchBuddy -configuration Debug build`.
- `windows/` — the cross-platform app this fork lives in: Tauri 2, Rust backend,
  TypeScript front end with no framework. **This is the one to touch.** It
  builds for Windows and Linux from the same tree; the version and the name
  (`coucou`) are the same on both.
- `docs/`, `design/` — behaviour and the visual source of truth (French). Visual
  changes belong to `design/prototype/notch-buddy.html` and `design/captures/`.

## Layout of `windows/`

```
src/                 island front end (TypeScript, no framework)
  mochi/             Mochi and the launch greeting, in Canvas 2D
  island/            state machine, hooks, integrations
  views/             every island view
  settings/          the settings window
src-tauri/           Rust backend: window, relay socket, Claude API, pollers
  src/platform/      everything that differs per OS; mod.rs is the shared facade
hook/                coucou-hook, the relay Claude Code and opencode talk to
opencode-plugin/     coucou.js, the opencode plugin (same relay, same socket)
```

## Build and test

```bash
cd windows
npm install
npm run build              # tsc --noEmit && vite build
npm run tauri dev          # live-reloading dev build
npm run pack               # installer on Windows, AppImage/.deb/.rpm on Linux
```

`predev`/`prebuild` build the release relay first, which the app copies on
launch; if a build complains that `coucou-hook` is missing, that is the step to
run by hand: `cargo build --release -p coucou-hook`.

Rust checks, verified on Windows:

```bash
cargo check --workspace --all-targets
cargo test -p coucou-hook            # the relay's tests; the only ones that run here
```

On this machine there is no MSVC `link.exe`, so cargo needs the GNU toolchain:
`cargo +stable-x86_64-pc-windows-gnu …`. `cargo test --workspace` then fails to
link the Tauri crate (`export ordinal too large`), which is a toolchain limit,
not a code fault — test `-p coucou-hook` and let CI cover the app.

## The rule you will otherwise break

`platform/linux.rs` and `hook/src/unix.rs` are `cfg(target_os = "linux")`:
**nothing on a Windows machine compiles them**, and neither does the upstream CI,
which only has Windows and macOS runners. Code written there is unverified until
it reaches Linux, and the first two Arch runs found a real `E0716` in minutes.

So: after touching anything under `cfg(target_os = "linux")`, push and watch the
Arch workflow instead of assuming.

```bash
git push origin arch-linux
gh run list --repo ZidaneLima/coucou --branch arch-linux --limit 3
gh run view <id> --repo ZidaneLima/coucou --log-failed
```

`.github/workflows/arch.yml` compiles and tests in an `archlinux` container;
`linux.yml` is the release job for Ubuntu.

## Rules

- No telemetry. Network calls only go to services the user configured.
- Secrets live in the OS keychain (`keyring`: Credential Manager on Windows,
  Secret Service on Linux) — never on disk, never in git.
- Never block Claude Code: if the app does not answer, the hook exits
  immediately. The relay's timeouts exist for this.
- Never overwrite `~/.claude/settings.json` outright: dated backup, merge, show
  the diff, write only after the user confirms.
- Never send an email or approve a permission without an explicit click.
- 0 % CPU while the island is hidden.
- Keep the bundle identifier `fr.louisraille.NotchBuddy`: Keychain items,
  preferences and permissions depend on it.
- No third-party dependencies unless truly unavoidable. Mochi is `Canvas` +
  `TimelineView`, no Rive/Lottie/images, and no front-end framework.
- No comments in code unless asked.

## Gotchas

- **Line endings.** `.gitattributes` pins every text file to LF; `core.autocrlf`
  is `true` here, so git prints "LF will be replaced by CRLF" on almost every
  add. That is expected. CRLF in `install.sh` or a `.desktop` file is a broken
  file on Linux, not a cosmetic difference.
- **The relay is one protocol, two transports.** `hook/src/main.rs` is shared;
  only `connect()` differs (named pipe / Unix socket). Both ends check that the
  other runs as the same user. Approval answers go back as the bare word
  `allow`/`deny`, and `coucou-hook` turns that into the `hookSpecificOutput`
  JSON the agent expects — keep that conversion in exactly one place.
- **opencode reuses the Claude Code hook shape**, plus `"agent": "opencode"`,
  and maps its own `permission.asked` onto `PermissionRequest` because that is
  the only event the relay waits on. The plugin installs to
  `$XDG_CONFIG_HOME/opencode/plugins/coucou.js` (`~/.config/opencode/plugins`).
- **The island is a gtk-layer-shell surface on the `top` layer**, not `overlay`:
  wlroots only grants keyboard interactivity on the top layer, so an overlay
  island can never be typed into. `COUCOU_LAYER`, `COUCOU_TOP_MARGIN`,
  `COUCOU_LAYER_SHELL` and `COUCOU_CURSOR_POLL` are the escape hatches. See
  `windows/README.md`.
- **The log** is at `~/.local/share/coucou/coucou.log` on Linux and
  `%LOCALAPPDATA%\Coucou\coucou.log` on Windows.
- `packaging/arch/install.sh` is the supported install; `PKGBUILD` still wants a
  release tarball and `sha256sums=('SKIP')` is a placeholder.
