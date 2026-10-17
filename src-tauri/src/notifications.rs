//! Desktop notifications on macOS. macOS 27 refuses the legacy NSUserNotification API (it logs
//! "Denying message … LegacyConnection" and nothing appears), so:
//!
//! - In the built app (a real .app bundle): UNUserNotificationCenter, in-process. Notifications show
//!   as Dev Inbox, permission is asked for once, they appear even while the app is in front, and a
//!   click opens the item.
//! - In `tauri dev` (a bare binary — UNUserNotificationCenter needs a bundle): AppleScript's
//!   `display notification`, which macOS still delivers. It shows as Script Editor and clicks can't
//!   be routed back, so it's for development only.

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{Bool, ProtocolObject};
use objc2::{define_class, msg_send, AllocAnyThread};
use objc2_foundation::{NSBundle, NSError, NSObject, NSObjectProtocol, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNAuthorizationStatus, UNMutableNotificationContent, UNNotification,
    UNNotificationPresentationOptions, UNNotificationRequest, UNNotificationResponse, UNNotificationSettings,
    UNNotificationSound, UNUserNotificationCenter, UNUserNotificationCenterDelegate,
};
use std::sync::OnceLock;

/// Separates the parts of a notification's identifier: id, link, item id.
const SEP: &str = "\u{1f}";

type OnClick = Box<dyn Fn(Option<String>, Option<String>) + Send + Sync>;
static ON_CLICK: OnceLock<OnClick> = OnceLock::new();

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "DevInboxNotificationDelegate"]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl UNUserNotificationCenterDelegate for Delegate {
        /// Show banners even while Dev Inbox is the frontmost app.
        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present(
            &self,
            _center: &UNUserNotificationCenter,
            _notification: &UNNotification,
            handler: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            handler.call((UNNotificationPresentationOptions::Banner
                | UNNotificationPresentationOptions::List
                | UNNotificationPresentationOptions::Sound,));
        }

        /// A click: hand the link and item to the app.
        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn did_receive(
            &self,
            _center: &UNUserNotificationCenter,
            response: &UNNotificationResponse,
            handler: &block2::DynBlock<dyn Fn()>,
        ) {
            let id = response.notification().request().identifier().to_string();
            let mut parts = id.split(SEP).skip(1).map(|s| (!s.is_empty()).then(|| s.to_string()));
            let (url, item) = (parts.next().flatten(), parts.next().flatten());
            if let Some(on_click) = ON_CLICK.get() {
                on_click(url, item);
            }
            handler.call(());
        }
    }
);

impl Delegate {
    fn new() -> Retained<Self> {
        let this = Self::alloc().set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}

/// Whether we're running from a real .app bundle (the built app), where the modern API works.
pub fn bundled() -> bool {
    NSBundle::mainBundle().bundlePath().to_string().ends_with(".app")
}

/// Set up: the click handler and, in the built app, the delegate and the permission prompt (once).
pub fn init(on_click: impl Fn(Option<String>, Option<String>) + Send + Sync + 'static) {
    let _ = ON_CLICK.set(Box::new(on_click));
    if !bundled() {
        return;
    }
    let center = UNUserNotificationCenter::currentNotificationCenter();
    let delegate = Delegate::new();
    center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    // The centre only holds the delegate weakly: keep it for the life of the app.
    std::mem::forget(delegate);
    let done = RcBlock::new(|granted: Bool, _error: *mut NSError| {
        if !granted.as_bool() {
            eprintln!("notifications: not allowed (System Settings → Notifications → Dev Inbox)");
        }
    });
    center.requestAuthorizationWithOptions_completionHandler(
        UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound | UNAuthorizationOptions::Badge,
        &done,
    );
}

/// How a notification was (or would be) sent, for Settings' "send a test".
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Route {
    /// The built app: shows as Dev Inbox, clicks open the item.
    App,
    /// The built app, but notifications are turned off for it in System Settings.
    Denied,
    /// `tauri dev`: AppleScript, shows as Script Editor, clicks don't come back.
    Dev,
}

/// Whether notifications are allowed for the built app (waits briefly for macOS to answer).
pub fn route() -> Route {
    if !bundled() {
        return Route::Dev;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let block = RcBlock::new(move |settings: std::ptr::NonNull<UNNotificationSettings>| {
        let status = unsafe { settings.as_ref() }.authorizationStatus();
        let _ = tx.send(status);
    });
    UNUserNotificationCenter::currentNotificationCenter().getNotificationSettingsWithCompletionHandler(&block);
    match rx.recv_timeout(std::time::Duration::from_secs(3)) {
        Ok(s) if s == UNAuthorizationStatus::Denied => Route::Denied,
        _ => Route::App,
    }
}

/// Show a notification. `url` opens on click; `item` is the app's id for it (marked seen on click).
pub fn send(title: &str, subtitle: &str, body: &str, url: Option<&str>, item: Option<&str>) {
    if !bundled() {
        dev_notify(title, subtitle, body);
        return;
    }
    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(title));
    if !subtitle.is_empty() {
        content.setSubtitle(&NSString::from_str(subtitle));
    }
    content.setBody(&NSString::from_str(body));
    content.setSound(Some(&UNNotificationSound::defaultSound()));
    let unique = format!("{:?}", std::time::SystemTime::now());
    let id = [unique.as_str(), url.unwrap_or(""), item.unwrap_or("")].join(SEP);
    let request = UNNotificationRequest::requestWithIdentifier_content_trigger(&NSString::from_str(&id), &content, None);
    let done = RcBlock::new(|error: *mut NSError| {
        if let Some(e) = unsafe { error.as_ref() } {
            eprintln!("notification not shown: {}", e.localizedDescription());
        }
    });
    UNUserNotificationCenter::currentNotificationCenter().addNotificationRequest_withCompletionHandler(&request, Some(&done));
}

/// `tauri dev`: AppleScript notifications still get through. Text goes in as arguments, never as script.
fn dev_notify(title: &str, subtitle: &str, body: &str) {
    let result = std::process::Command::new("osascript")
        .args([
            "-e",
            "on run argv",
            "-e",
            "display notification (item 3 of argv) with title (item 1 of argv) subtitle (item 2 of argv)",
            "-e",
            "end run",
            title,
            subtitle,
            body,
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    if let Err(e) = result {
        eprintln!("notification not shown: {e}");
    }
}
