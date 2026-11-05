//! macOS integration through AppKit: files from Finder's "Open With", Dock
//! drops and `open` (the `odoc` Apple Event), the "Compress with ZipShrink"
//! Services entry, Quit from the Dock or at logout (the `quit` Apple Event,
//! routed through the window's close path so a running job asks first), and
//! the menu bar. AppKit calls arrive on the main thread outside egui's frame;
//! they are queued here and the app takes them in `App::logic`, which runs
//! before each frame and also while the window is minimized or hidden.
//!
//! Files that arrive through these routes count as the user's own choice,
//! so macOS asks no extra privacy question before ZipShrink reads them.

use crate::app::{Cmd, Page};
use eframe::egui;
use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol, Sel};
use objc2::{define_class, msg_send, sel, ClassType, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSApplication, NSApplicationWillFinishLaunchingNotification, NSEventModifierFlags,
    NSMenu, NSMenuItem, NSPasteboard, NSUpdateDynamicServices, NSWorkspace,
};
use objc2_foundation::{
    NSAppleEventDescriptor, NSAppleEventManager, NSArray, NSNotification, NSNotificationCenter, NSString, NSUserDefaults, NSURL,
};
use std::cell::OnceCell;
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// Something AppKit asked for, handled by the app on its next frame.
pub enum Event {
    /// Files and folders to compress (or jobs to resume), as one batch.
    Open(Vec<PathBuf>),
    Command(Cmd),
    /// The outcome of "Install Command Line Tool…" (nothing when cancelled).
    CliInstalled(Result<(), String>),
}

static QUEUE: Mutex<Vec<Event>> = Mutex::new(Vec::new());
static CTX: OnceLock<egui::Context> = OnceLock::new();

fn push(e: Event) {
    if let Ok(mut q) = QUEUE.lock() {
        q.push(e);
    }
    if let Some(ctx) = CTX.get() {
        ctx.request_repaint();
    }
}

/// Lets queued events wake the window.
pub fn attach(ctx: &egui::Context) {
    let _ = CTX.set(ctx.clone());
    ctx.request_repaint();
}

/// Everything queued since the last call.
pub fn take() -> Vec<Event> {
    QUEUE.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default()
}

const fn four_cc(c: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*c)
}
const CORE_EVENT_CLASS: u32 = four_cc(b"aevt");
const OPEN_DOCUMENTS: u32 = four_cc(b"odoc");
const QUIT_APPLICATION: u32 = four_cc(b"quit");
const DIRECT_OBJECT: u32 = four_cc(b"----");

/// The app's own menu commands; a menu item's tag is its index here.
const MENU: [Cmd; 10] = [
    Cmd::Open,
    Cmd::OpenFolder,
    Cmd::Close,
    Cmd::Quit,
    Cmd::Go(Page::Compress),
    Cmd::Go(Page::Verify),
    Cmd::Go(Page::Resume),
    Cmd::Go(Page::Preferences),
    Cmd::InstallCli,
    Cmd::Help,
];

define_class!(
    // SAFETY: NSObject has no subclassing requirements and Bridge does not
    // implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "ZipShrinkBridge"]
    struct Bridge;

    unsafe impl NSObjectProtocol for Bridge {}

    impl Bridge {
        #[unsafe(method(applicationWillFinishLaunching:))]
        fn will_finish_launching(&self, _note: &NSNotification) {
            self.install_handlers();
        }

        #[unsafe(method(handleOpenDocuments:withReplyEvent:))]
        fn open_documents(&self, event: &NSAppleEventDescriptor, _reply: &NSAppleEventDescriptor) {
            // SAFETY: paramDescriptorForKeyword: takes an AEKeyword (UInt32)
            // and returns an autoreleased descriptor or nil.
            let list: Option<Retained<NSAppleEventDescriptor>> = unsafe { msg_send![event, paramDescriptorForKeyword: DIRECT_OBJECT] };
            let mut paths = Vec::new();
            if let Some(list) = list {
                for i in 1..=list.numberOfItems() {
                    if let Some(d) = list.descriptorAtIndex(i) {
                        paths.extend(d.fileURLValue().as_deref().and_then(url_path));
                    }
                }
                // A single file may come as a plain file URL, not a list.
                if paths.is_empty() {
                    paths.extend(list.fileURLValue().as_deref().and_then(url_path));
                }
            }
            if !paths.is_empty() {
                push(Event::Open(paths));
            }
        }

        #[unsafe(method(handleQuit:withReplyEvent:))]
        fn quit(&self, _event: &NSAppleEventDescriptor, _reply: &NSAppleEventDescriptor) {
            push(Event::Command(Cmd::Quit));
        }

        /// The Services entry (NSMessage `compressFiles` in Info.plist).
        #[unsafe(method(compressFiles:userData:error:))]
        fn compress_files(&self, pboard: &NSPasteboard, _user_data: Option<&NSString>, _error: *mut *mut NSString) {
            let classes = NSArray::from_slice(&[NSURL::class()]);
            // SAFETY: asks for NSURL objects, with no options.
            let found = unsafe { pboard.readObjectsForClasses_options(&classes, None) };
            let paths: Vec<PathBuf> = found
                .map(|a| a.to_vec())
                .unwrap_or_default()
                .iter()
                .filter_map(|o| o.downcast_ref::<NSURL>().and_then(url_path))
                .collect();
            if !paths.is_empty() {
                push(Event::Open(paths));
            }
        }

        #[unsafe(method(menuCommand:))]
        fn menu_command(&self, sender: Option<&NSMenuItem>) {
            if let Some(cmd) = sender.and_then(|s| usize::try_from(s.tag()).ok()).and_then(|i| MENU.get(i)) {
                push(Event::Command(*cmd));
            }
        }
    }
);

impl Bridge {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: NSObject's init.
        unsafe { msg_send![super(this), init] }
    }

    fn install_handlers(&self) {
        let mgr = NSAppleEventManager::sharedAppleEventManager();
        let handlers = [(sel!(handleOpenDocuments:withReplyEvent:), OPEN_DOCUMENTS), (sel!(handleQuit:withReplyEvent:), QUIT_APPLICATION)];
        for (selector, id) in handlers {
            // SAFETY: the handler lives for the whole process (see `bridge`),
            // the selectors name methods defined above with the
            // (event, reply) signature, and class and ID are FourCharCodes.
            unsafe {
                let _: () = msg_send![&*mgr, setEventHandler: self, andSelector: selector, forEventClass: CORE_EVENT_CLASS, andEventID: id];
            }
        }
        let app = NSApplication::sharedApplication(self.mtm());
        // SAFETY: Bridge implements compressFiles:userData:error:.
        unsafe { app.setServicesProvider(Some(self)) };
        // Registers the Services entry without waiting for a log-out.
        NSUpdateDynamicServices();
    }
}

thread_local! {
    static BRIDGE: OnceCell<&'static Bridge> = const { OnceCell::new() };
}

/// The one Bridge object. AppKit keeps unretained references to it (Apple
/// Event handler, menu targets), so it is never released.
fn bridge(mtm: MainThreadMarker) -> &'static Bridge {
    BRIDGE.with(|b| {
        *b.get_or_init(|| {
            let raw = Retained::into_raw(Bridge::new(mtm));
            // SAFETY: the pointer comes from a Retained that is never released.
            unsafe { &*raw }
        })
    })
}

fn url_path(url: &NSURL) -> Option<PathBuf> {
    if !url.isFileURL() {
        return None;
    }
    url.path().map(|p| PathBuf::from(p.to_string()))
}

/// Call once on the main thread before the event loop starts. AppKit
/// installs its own `odoc` and `quit` handlers while launching; ours replace
/// them when it announces that it will finish launching, before the first
/// Apple Event is dispatched.
pub fn install_early() {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let b = bridge(mtm);
    // SAFETY: the observer is never released, and the selector matches the
    // notification method defined above.
    unsafe {
        NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
            b,
            sel!(applicationWillFinishLaunching:),
            Some(NSApplicationWillFinishLaunchingNotification),
            None,
        );
    }
}

fn add_item(
    mtm: MainThreadMarker,
    menu: &NSMenu,
    title: &str,
    action: Option<Sel>,
    key: &str,
    mods: NSEventModifierFlags,
) -> Retained<NSMenuItem> {
    // SAFETY: the actions are standard responder-chain selectors or ours.
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(mtm.alloc(), &NSString::from_str(title), action, &NSString::from_str(key))
    };
    if !key.is_empty() {
        item.setKeyEquivalentModifierMask(mods);
    }
    menu.addItem(&item);
    item
}

fn add_command(mtm: MainThreadMarker, menu: &NSMenu, title: &str, key: &str, mods: NSEventModifierFlags, cmd: Cmd) {
    let item = add_item(mtm, menu, title, Some(sel!(menuCommand:)), key, mods);
    // SAFETY: the bridge implements menuCommand: and is never released.
    unsafe { item.setTarget(Some(bridge(mtm))) };
    item.setTag(MENU.iter().position(|c| *c == cmd).map_or(-1, |i| i as isize));
}

fn add_separator(mtm: MainThreadMarker, menu: &NSMenu) {
    menu.addItem(&NSMenuItem::separatorItem(mtm));
}

/// The menu bar: ZipShrink, File, View, Window and Help. Replaces winit's
/// default menu, whose Quit would end the app without asking.
pub fn install_menu() {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let app = NSApplication::sharedApplication(mtm);
    let bar = NSMenu::new(mtm);
    let submenu = |title: &str| {
        let menu = NSMenu::initWithTitle(mtm.alloc(), &NSString::from_str(title));
        let item = NSMenuItem::new(mtm);
        item.setSubmenu(Some(&menu));
        bar.addItem(&item);
        menu
    };
    let cmd = NSEventModifierFlags::Command;
    let shift_cmd = NSEventModifierFlags::Command | NSEventModifierFlags::Shift;
    let none = NSEventModifierFlags::empty();

    let m = submenu("ZipShrink");
    add_item(mtm, &m, "About ZipShrink", Some(sel!(orderFrontStandardAboutPanel:)), "", none);
    add_separator(mtm, &m);
    add_command(mtm, &m, "Preferences…", ",", cmd, Cmd::Go(Page::Preferences));
    add_command(mtm, &m, "Install Command Line Tool…", "", none, Cmd::InstallCli);
    add_separator(mtm, &m);
    let services = NSMenu::initWithTitle(mtm.alloc(), &NSString::from_str("Services"));
    add_item(mtm, &m, "Services", None, "", none).setSubmenu(Some(&services));
    app.setServicesMenu(Some(&services));
    add_separator(mtm, &m);
    add_item(mtm, &m, "Hide ZipShrink", Some(sel!(hide:)), "h", cmd);
    add_item(mtm, &m, "Hide Others", Some(sel!(hideOtherApplications:)), "h", cmd | NSEventModifierFlags::Option);
    add_item(mtm, &m, "Show All", Some(sel!(unhideAllApplications:)), "", none);
    add_separator(mtm, &m);
    add_command(mtm, &m, "Quit ZipShrink", "q", cmd, Cmd::Quit);

    let m = submenu("File");
    add_command(mtm, &m, "Open…", "o", cmd, Cmd::Open);
    add_command(mtm, &m, "Add Folder…", "o", shift_cmd, Cmd::OpenFolder);
    add_separator(mtm, &m);
    add_command(mtm, &m, "Close Window", "w", cmd, Cmd::Close);

    let m = submenu("View");
    add_command(mtm, &m, "Compress", "1", cmd, Cmd::Go(Page::Compress));
    add_command(mtm, &m, "Verify", "2", cmd, Cmd::Go(Page::Verify));
    add_command(mtm, &m, "Resume", "3", cmd, Cmd::Go(Page::Resume));

    let m = submenu("Window");
    add_item(mtm, &m, "Minimize", Some(sel!(performMiniaturize:)), "m", cmd);
    add_item(mtm, &m, "Zoom", Some(sel!(performZoom:)), "", none);
    add_separator(mtm, &m);
    add_item(mtm, &m, "Bring All to Front", Some(sel!(arrangeInFront:)), "", none);
    app.setWindowsMenu(Some(&m));

    let m = submenu("Help");
    add_command(mtm, &m, "ZipShrink Help", "", none, Cmd::Help);
    app.setHelpMenu(Some(&m));

    app.setMainMenu(Some(&bar));
}

/// Shows the app again after Hide (⌘H) and brings it to the front, so a
/// question or newly handed-over files are seen. The window itself is
/// brought back from the Dock with egui's viewport commands.
pub fn reveal() {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let app = NSApplication::sharedApplication(mtm);
    app.unhide(None);
    #[allow(deprecated)] // activate() needs macOS 14
    app.activateIgnoringOtherApps(true);
}

/// The "Reduce motion" accessibility setting.
pub fn reduce_motion() -> bool {
    NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion()
}

fn global_string(key: &str) -> Option<String> {
    NSUserDefaults::standardUserDefaults().stringForKey(&NSString::from_str(key)).map(|s| s.to_string())
}

/// Whether the system appearance is Dark (the app's own appearance is never
/// forced, only the window's).
pub fn system_dark() -> bool {
    let Some(mtm) = MainThreadMarker::new() else { return false };
    // SAFETY: AppKit's constant appearance names.
    let (aqua, dark) = unsafe { (NSAppearanceNameAqua, NSAppearanceNameDarkAqua) };
    let names = NSArray::from_slice(&[aqua, dark]);
    let best = NSApplication::sharedApplication(mtm).effectiveAppearance().bestMatchFromAppearancesWithNames(&names);
    best.is_some_and(|n| n.isEqualToString(dark))
}

/// What double-clicking a title bar does (Desktop & Dock settings).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TitleDoubleClick {
    Zoom,
    Minimize,
    Nothing,
}

pub fn title_double_click() -> TitleDoubleClick {
    match global_string("AppleActionOnDoubleClick").as_deref() {
        Some("Minimize") => TitleDoubleClick::Minimize,
        Some("None") => TitleDoubleClick::Nothing,
        _ => TitleDoubleClick::Zoom,
    }
}

/// `ZipShrink.app` around this executable (`Contents/MacOS/zipshrink-gui`),
/// with symlinks resolved.
fn bundle_root() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    let root = exe.parent()?.parent()?.parent()?;
    root.extension().is_some_and(|e| e == "app").then(|| root.to_path_buf())
}

/// Run from a random read-only copy (an app opened straight from Downloads
/// or the disk image): nothing written there lasts.
fn translocated(root: &Path) -> bool {
    root.to_string_lossy().contains("/AppTranslocation/")
}

fn c_path(p: &Path) -> Option<CString> {
    CString::new(p.as_os_str().as_bytes()).ok()
}

/// Removes `com.apple.quarantine` from the app's own bundle, so the bundled
/// command line tool (and the updater's version check of it) is not blocked
/// by Gatekeeper separately once the user has opened the app. Only for a
/// bundle that is writable and not translocated; runs in the background.
pub fn clear_own_quarantine() {
    let Some(root) = bundle_root() else { return };
    if translocated(&root) {
        return;
    }
    let Some(c) = c_path(&root) else { return };
    // SAFETY: a valid NUL-terminated path.
    if unsafe { libc::access(c.as_ptr(), libc::W_OK) } != 0 {
        return;
    }
    let _ = std::thread::Builder::new().name("unquarantine".into()).spawn(move || unquarantine_tree(&root));
}

fn unquarantine_tree(p: &Path) {
    if let Some(c) = c_path(p) {
        // SAFETY: valid NUL-terminated strings; failure (no such attribute,
        // not ours) is ignored.
        unsafe { libc::removexattr(c.as_ptr(), c"com.apple.quarantine".as_ptr(), libc::XATTR_NOFOLLOW) };
    }
    let is_dir = std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir());
    if is_dir {
        if let Ok(rd) = std::fs::read_dir(p) {
            for e in rd.flatten() {
                unquarantine_tree(&e.path());
            }
        }
    }
}

/// "Install Command Line Tool…": links /usr/local/bin/zipshrink to the CLI
/// inside the bundle, asking for an administrator password.
pub fn install_cli() {
    let root = bundle_root();
    let cli = root.as_ref().map(|r| r.join("Contents/MacOS/zipshrink"));
    let Some(cli) = cli.filter(|c| c.is_file()) else {
        push(Event::CliInstalled(Err("This copy of ZipShrink has no command line tool. Install ZipShrink from its disk image.".into())));
        return;
    };
    if root.as_deref().is_some_and(translocated) {
        push(Event::CliInstalled(Err("Move ZipShrink to the Applications folder first, then open it from there.".into())));
        return;
    }
    let _ = std::thread::Builder::new().name("install-cli".into()).spawn(move || {
        // The path goes in as an argument and through `quoted form of`, so
        // no quoting can break the shell command.
        let script = [
            "on run argv",
            "do shell script \"mkdir -p /usr/local/bin && ln -sf \" & quoted form of (item 1 of argv) & \" /usr/local/bin/zipshrink\" with administrator privileges",
            "end run",
        ];
        let mut cmd = std::process::Command::new("/usr/bin/osascript");
        for line in script {
            cmd.arg("-e").arg(line);
        }
        let result = match cmd.arg(&cli).output() {
            Ok(o) if o.status.success() => Ok(()),
            // -128: the user cancelled the password prompt.
            Ok(o) if String::from_utf8_lossy(&o.stderr).contains("-128") => return,
            Ok(o) => Err(String::from_utf8_lossy(&o.stderr).trim().to_string()),
            Err(e) => Err(e.to_string()),
        };
        push(Event::CliInstalled(result));
    });
}
