// Linux: XDG directories for files, xdg-open for links and folders, and
// gtk-layer-shell for the island window.
//
// Wayland gives an app no global cursor position and no say over where its
// window goes, so the island works differently from Windows:
//   * it is a layer-shell surface anchored to the top edge, on the top layer
//     (wlroots — Hyprland, Sway — only hands out the keyboard there), taking no
//     exclusive space so no window is ever pushed around;
//   * click-through is the window's input region, set to the island shape, so
//     the compositor itself sends every other click to whatever is underneath;
//   * on Hyprland the compositor's own event socket supplies the cursor, so the
//     island behaves exactly like the Windows one. Everywhere else the pointer
//     is only known while it is over the island, and the page reports it.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use gtk::glib::translate::ToGlibPtr;
use gtk::prelude::*;
use tauri::{AppHandle, WebviewWindow};

use super::{home_dir, LocalTime};

/// File name of the Claude Code relay.
pub const HOOK_EXE: &str = "coucou-hook";

/// Environment variable holding the home directory.
pub const HOME_VAR: &str = "HOME";

// ── Files ─────────────────────────────────────────────────────────────────────

/// An XDG base directory (`$XDG_CONFIG_HOME` …), or its fallback under the home
/// directory when it is unset or not absolute.
fn xdg(var: &str, fallback: &str) -> PathBuf {
    std::env::var_os(var)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home_dir().join(fallback))
}

/// ~/.config/coucou — preferences.
pub fn config_dir() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config").join("coucou")
}

/// ~/.local/share/coucou — where coucou-hook, the inbox and the log live. The
/// relay has to sit at a stable path: an AppImage is mounted somewhere new on
/// every launch.
pub fn local_dir() -> PathBuf {
    xdg("XDG_DATA_HOME", ".local/share").join("coucou")
}

/// Environment the webview must inherit, set before any thread or process
/// starts.
///
/// Inside an AppImage, WebKit uses the GStreamer bundled with it, and GStreamer
/// keeps its plugin registry in ~/.cache/gstreamer-1.0 by default — the same
/// file the system's GStreamer uses. The AppImage is mounted somewhere new on
/// every launch, so each launch would rewrite the system's registry with
/// plugin paths that vanish once Coucou quits. Give ours its own file.
pub fn prepare_environment() {
    if std::env::var_os("APPIMAGE").is_none() || std::env::var_os("GST_REGISTRY").is_some() {
        return;
    }
    let cache = xdg("XDG_CACHE_HOME", ".cache").join("coucou");
    if std::fs::create_dir_all(&cache).is_ok() {
        std::env::set_var("GST_REGISTRY", cache.join("gstreamer-registry.bin"));
    }
}

pub fn local_time() -> LocalTime {
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        libc::localtime_r(&now, &mut tm);
    }
    LocalTime {
        year: (tm.tm_year + 1900) as u32,
        month: (tm.tm_mon + 1) as u32,
        day: tm.tm_mday as u32,
        hour: tm.tm_hour as u32,
        minute: tm.tm_min as u32,
        second: tm.tm_sec as u32,
    }
}

/// Where coucou-hook finds us: `$XDG_RUNTIME_DIR/coucou.sock`, or
/// `/run/user/<uid>/coucou.sock` when the variable is missing. Must match
/// `socket_path()` in hook/src/unix.rs exactly.
pub fn relay_socket_path() -> Option<PathBuf> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| {
            let p = PathBuf::from(format!("/run/user/{}", unsafe { libc::getuid() }));
            p.is_dir().then_some(p)
        })?;
    Some(dir.join("coucou.sock"))
}

// ── Processes ─────────────────────────────────────────────────────────────────

/// Nothing to hide: a spawned process only gets a terminal if it asks for one.
pub fn no_console(cmd: &mut Command) -> &mut Command {
    cmd
}

/// Only http(s): the string comes from the page, and `xdg-open` would happily
/// hand a `file://` URL or a `.desktop` file to whatever claims it.
pub fn open_url(url: &str) {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return;
    }
    let _ = Command::new("xdg-open").arg(url).spawn();
}

pub fn reveal_folder(path: &str) {
    let _ = Command::new("xdg-open").arg(path).spawn();
}

/// "Open terminal" opens the working folder in VS Code when `code` is on PATH,
/// falls back to $VISUAL/$EDITOR, and finally to the file manager.
///
/// No shell anywhere: the path is a project folder chosen by whoever is running
/// Claude Code, and `$EDITOR` is very often `code -w` or `nvim`, which a naive
/// split on the first space would mangle. The program is taken as one word and
/// everything after it as arguments, so `emacsclient -nw` works as written.
pub fn open_in_editor(path: Option<&str>) -> bool {
    let target = path.filter(|p| !p.is_empty());

    if let Some(code) = find_on_path("code") {
        let mut cmd = Command::new(code);
        if let Some(p) = target {
            cmd.arg(p);
        }
        if cmd.spawn().is_ok() {
            return true;
        }
    }

    for var in ["COUCOU_EDITOR", "VISUAL", "EDITOR"] {
        let Some(value) = std::env::var_os(var).filter(|v| !v.is_empty()) else { continue };
        let mut parts = value.to_string_lossy().split_whitespace();
        let Some(program) = parts.next() else { continue };
        let mut cmd = Command::new(program);
        cmd.args(parts);
        if let Some(p) = target {
            cmd.arg(p);
        }
        if cmd.spawn().is_ok() {
            return true;
        }
    }

    match target {
        Some(p) => {
            reveal_folder(p);
            true
        }
        None => false,
    }
}

/// Our own `which`: the first executable file named `stem` on $PATH.
pub fn find_on_path(stem: &str) -> Option<PathBuf> {
    let dirs = std::env::var_os("PATH")?;
    std::env::split_paths(&dirs)
        .map(|dir| dir.join(stem))
        .find(|p| {
            std::fs::metadata(p)
                .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
                .unwrap_or(false)
        })
}

// ── Cursor ────────────────────────────────────────────────────────────────────

// Wayland hands a normal client no global cursor position, so Mochi's eyes can
// only follow the pointer while it is over the island. Hyprland is the
// exception: its event socket streams `cursorpos` for every move, which gives us
// the same 60 Hz feed the Windows poll reads from Win32 — and with it the eyes
// that track the pointer across the whole screen.

/// Latest position Hyprland pushed, in physical screen pixels.
static CURSOR: Mutex<Option<(f64, f64)>> = Mutex::new(None);

/// Whether a global cursor source exists. Decided once, before the page is told
/// which source it has: the page either reports the pointer from its own mouse
/// events or stays out of the way, and two feeds must never drive the eyes.
static CURSOR_POLL: AtomicBool = AtomicBool::new(false);

fn hyprland_socket() -> Option<PathBuf> {
    let signature = std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE")?;
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from)?;
    let path = dir.join("hypr").join(signature).join(".socket2.sock");
    path.exists().then_some(path)
}

/// Opens the cursor feed if this is Hyprland. Called once at startup, before the
/// webview exists: a local Unix socket either answers at once or is not there,
/// so nothing here can delay a launch. `COUCOU_CURSOR_POLL=0` forces the
/// compositor-blind path back on.
pub fn start_cursor_feed() {
    if std::env::var("COUCOU_CURSOR_POLL")
        .map(|v| v == "0")
        .unwrap_or(false)
    {
        return;
    }
    let Some(path) = hyprland_socket() else { return };
    let Ok(stream) = std::os::unix::net::UnixStream::connect(&path) else {
        crate::log::line("Hyprland event socket refused the connection".to_string());
        return;
    };
    CURSOR_POLL.store(true, Ordering::Relaxed);
    crate::log::line("cursor feed: Hyprland event socket".to_string());
    std::thread::spawn(move || read_hyprland_events(path, stream));
}

/// `cursorpos >> 1234,567` is the only line we care about; everything else
/// (workspace changes, monitor hotplug, openwindows) is read and dropped.
///
/// The socket closes when Hyprland restarts and reappears at the same path, so
/// a dropped connection is retried forever instead of leaving Mochi's eyes
/// frozen until the app is relaunched.
fn read_hyprland_events(path: PathBuf, stream: std::os::unix::net::UnixStream) {
    use std::io::BufRead;

    let mut current = stream;
    let mut reported = false;

    loop {
        // Rebuilt on every (re)connect: a BufReader over a closed socket can
        // never see the new one.
        let mut reader = std::io::BufReader::new(match current.try_clone() {
            Ok(clone) => clone,
            Err(err) => {
                crate::log::line(format!("cursor feed: {err}"));
                return;
            }
        });

        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                // 0 bytes or an error: Hyprland is gone or restarted.
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }

            let Some(rest) = line.split_once(">>").map(|(_, rest)| rest.trim()) else {
                continue;
            };
            let Some((x, y)) = rest.split_once(',') else { continue };
            let (Ok(x), Ok(y)) = (x.trim().parse::<f64>(), y.trim().parse::<f64>()) else {
                continue;
            };
            *CURSOR.lock().unwrap() = Some((x, y));
        }

        // The socket comes back at the same path after a compositor restart.
        // While it is gone we retry quietly: a desktop whose compositor is
        // restarting is not news, and a log line every two seconds is a lot of
        // news.
        loop {
            match std::os::unix::net::UnixStream::connect(&path) {
                Ok(stream) => {
                    if reported {
                        crate::log::line("cursor feed: reconnected".to_string());
                    }
                    current = stream;
                    reported = false;
                    break;
                }
                Err(_) => {
                    if !reported {
                        crate::log::line(
                            "cursor feed: Hyprland event socket closed".to_string(),
                        );
                        reported = true;
                    }
                    std::thread::sleep(Duration::from_secs(2));
                }
            }
        }
    }
}

/// True when `cursor_physical` is worth reading: Win32 always, Hyprland when
/// the event socket opened, nowhere else.
pub fn cursor_poll() -> bool {
    CURSOR_POLL.load(Ordering::Relaxed)
}

/// Cursor position in physical screen pixels.
pub fn cursor_physical() -> Option<(f64, f64)> {
    *CURSOR.lock().unwrap()
}

pub fn left_button_down() -> bool {
    false
}

// ── Island window ─────────────────────────────────────────────────────────────

/// The few gtk-layer-shell calls we need, straight from the C library.
mod layer {
    use gtk::ffi::GtkWindow;
    use std::os::raw::{c_char, c_int};

    pub const LAYER_BACKGROUND: c_int = 0;
    pub const LAYER_BOTTOM: c_int = 1;
    pub const LAYER_TOP: c_int = 2;
    pub const LAYER_OVERLAY: c_int = 3;
    pub const EDGE_TOP: c_int = 2;
    pub const KEYBOARD_NONE: c_int = 0;
    pub const KEYBOARD_EXCLUSIVE: c_int = 1;

    #[link(name = "gtk-layer-shell")]
    extern "C" {
        pub fn gtk_layer_is_supported() -> c_int;
        pub fn gtk_layer_init_for_window(window: *mut GtkWindow);
        pub fn gtk_layer_set_namespace(window: *mut GtkWindow, name_space: *const c_char);
        pub fn gtk_layer_set_layer(window: *mut GtkWindow, layer: c_int);
        pub fn gtk_layer_set_anchor(window: *mut GtkWindow, edge: c_int, anchor: c_int);
        pub fn gtk_layer_set_margin(window: *mut GtkWindow, edge: c_int, margin: c_int);
        pub fn gtk_layer_set_exclusive_zone(window: *mut GtkWindow, zone: c_int);
        pub fn gtk_layer_set_keyboard_mode(window: *mut GtkWindow, mode: c_int);
    }
}

/// Which layer the island lives in.
///
/// `top` is the default, and on Hyprland (wlroots) it is the only one that can
/// hold the keyboard: wlroots hands out keyboard interactivity on the top layer
/// and ignores it anywhere else, so an overlay island can never be typed into.
/// The cost is that we share the layer with the bar — Waybar, or whatever
/// Omarchy draws up there — and layer surfaces keep their mapping order, so a
/// bar that reloads afterwards ends up drawn over the island.
///
/// `COUCOU_LAYER=overlay` puts us above everything, which looks better on
/// compositors that do allow keyboard focus there (COSMIC, KDE Plasma); the chat
/// box then has to be opened with the mouse.
fn island_layer() -> i32 {
    match std::env::var("COUCOU_LAYER").unwrap_or_default().as_str() {
        "background" => layer::LAYER_BACKGROUND,
        "bottom" => layer::LAYER_BOTTOM,
        "overlay" => layer::LAYER_OVERLAY,
        _ => layer::LAYER_TOP,
    }
}

/// Logical-pixel gap between the screen's top edge and the island, for a bar
/// that would otherwise be drawn over Mochi's head. `COUCOU_TOP_MARGIN=40`
/// clears a 40 px bar.
fn top_margin() -> i32 {
    std::env::var("COUCOU_TOP_MARGIN")
        .ok()
        .and_then(|v| v.trim().parse::<i32>().ok())
        .unwrap_or(0)
        .max(0)
}

/// True once the island window is a layer-shell surface.
static LAYER_SURFACE: AtomicBool = AtomicBool::new(false);

/// The input region last asked for, re-applied whenever the window is mapped:
/// GTK resets it to the whole window on map. Until the page reports the island
/// shape it is empty, so nothing takes the mouse.
type Region = Option<(f64, f64, f64, f64)>;
static INPUT_REGION: Mutex<Region> = Mutex::new(Some((0.0, 0.0, 0.0, 0.0)));

fn gtk_window_ptr(win: &gtk::ApplicationWindow) -> *mut gtk::ffi::GtkWindow {
    let w: &gtk::Window = win.upcast_ref();
    w.to_glib_none().0
}

/// WebKitGTK has no competing drop target to remove.
pub fn unblock_webview_drops(_app: &AppHandle) {}

/// Turns the island into a layer-shell surface on the top edge that never takes
/// the keyboard until something asks for it. Must run before the window is
/// first shown: a layer surface cannot be made out of a window the compositor
/// already knows.
///
/// Without layer-shell (GNOME, X11, or COUCOU_LAYER_SHELL=0) the window stays
/// an ordinary always-on-top window that refuses focus; where it lands is then
/// up to the window manager.
pub fn make_non_activating(win: &WebviewWindow) {
    let Ok(gw) = win.gtk_window() else { return };
    // COUCOU_LAYER_SHELL=0 is the way out on a compositor where it misbehaves.
    let wanted = std::env::var("COUCOU_LAYER_SHELL").map(|v| v != "0").unwrap_or(true);
    let supported = unsafe { layer::gtk_layer_is_supported() } != 0;
    if !wanted || !supported || gw.is_realized() {
        let why = if !wanted {
            "COUCOU_LAYER_SHELL=0"
        } else if supported {
            "window already shown"
        } else {
            "compositor has no layer-shell"
        };
        crate::log::line(format!("island is a regular window ({why})"));
        gw.set_accept_focus(false);
        return;
    }
    // tao gives undecorated Wayland windows an empty titlebar to force
    // client-side decorations. A layer surface has none, and a client-decorated
    // GtkWindow recomputes its own input region (shadow margins included) on
    // every map, over ours.
    gw.set_titlebar(None::<&gtk::Widget>);
    let which = island_layer();
    let margin = top_margin();
    let ptr = gtk_window_ptr(&gw);
    unsafe {
        layer::gtk_layer_init_for_window(ptr);
        layer::gtk_layer_set_namespace(ptr, c"coucou".as_ptr());
        layer::gtk_layer_set_layer(ptr, which);
        // Top edge only: the compositor centres the surface horizontally.
        layer::gtk_layer_set_anchor(ptr, layer::EDGE_TOP, 1);
        layer::gtk_layer_set_margin(ptr, layer::EDGE_TOP, margin);
        // 0, never -1. A negative exclusive zone means "reserve my own height",
        // which on Hyprland pushes every window on the workspace 320 px down to
        // make room for a floating island. Coucou is an overlay: it takes no
        // space from anything.
        layer::gtk_layer_set_exclusive_zone(ptr, 0);
        layer::gtk_layer_set_keyboard_mode(ptr, layer::KEYBOARD_NONE);
    }
    // WebKitGTK in a freshly mapped layer surface never paints its first frame
    // (seen on COSMIC, and reproduced with a bare GTK window + WebKitGTK, no
    // Tauri involved): the surface stays empty. Unmapping and mapping it once,
    // right after the first map, gets it drawing for good.
    let remapped = std::cell::Cell::new(false);
    gw.connect_map_event(move |w, _| {
        apply_input_region(w, *INPUT_REGION.lock().unwrap());
        if !remapped.replace(true) {
            let w = w.clone();
            gtk::glib::idle_add_local_once(move || {
                w.hide();
                w.show_all();
                apply_input_region(&w, *INPUT_REGION.lock().unwrap());
            });
        }
        gtk::glib::Propagation::Proceed
    });
    LAYER_SURFACE.store(true, Ordering::Relaxed);
    crate::log::line(match which {
        layer::LAYER_OVERLAY => "island is a layer-shell overlay".to_string(),
        layer::LAYER_TOP => format!("island is a layer-shell top-layer surface (margin {margin}px)"),
        other => format!("island is a layer-shell surface (layer {other})"),
    });
}

/// Temporarily allow keyboard focus so a text field inside the island can be
/// typed in.
///
/// EXCLUSIVE, not ON_DEMAND: on-demand only grabs the keyboard once the user
/// clicks the surface, and the island asks for focus from JavaScript the moment
/// a text field is focused — which on a bar you click once is not the same
/// thing. Exclusive also matches what every other layer-shell panel does. The
/// compositor drops the grab as soon as another window takes focus, so nothing
/// is stolen from the terminal behind.
pub fn set_activating(win: &WebviewWindow, activating: bool) {
    let Ok(gw) = win.gtk_window() else { return };
    if LAYER_SURFACE.load(Ordering::Relaxed) {
        let mode = if activating {
            layer::KEYBOARD_EXCLUSIVE
        } else {
            layer::KEYBOARD_NONE
        };
        unsafe { layer::gtk_layer_set_keyboard_mode(gtk_window_ptr(&gw), mode) };
    } else {
        gw.set_accept_focus(activating);
    }
}

/// Only this rectangle (window-logical pixels) takes the mouse; `None` means
/// the whole window does. Everything outside goes to the window underneath.
pub fn set_input_region(win: &WebviewWindow, rect: Region) {
    *INPUT_REGION.lock().unwrap() = rect;
    let Ok(gw) = win.gtk_window() else { return };
    apply_input_region(&gw, rect);
}

fn apply_input_region(gw: &impl IsA<gtk::Widget>, rect: Region) {
    match rect {
        None => gw.input_shape_combine_region(None),
        Some((x, y, w, h)) => {
            let Some(gdk_window) = gw.window() else { return };
            let region = gtk::cairo::Region::create_rectangle(&gtk::cairo::RectangleInt::new(
                x.floor() as i32,
                y.floor() as i32,
                w.ceil().max(0.0) as i32,
                h.ceil().max(0.0) as i32,
            ));
            gdk_window.input_shape_combine_region(&region, 0, 0);
        }
    }
}
