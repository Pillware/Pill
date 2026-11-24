//! Show and hide a docked view's child window on macOS without making it key.

use objc2::rc::Retained;
use objc2_app_kit::{NSView, NSWindow, NSWindowOrderingMode};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

/// The AppKit window behind a winit window.
pub(super) fn ns_window(window: &Window) -> Option<Retained<NSWindow>> {
    let RawWindowHandle::AppKit(handle) = window.window_handle().ok()?.as_raw() else {
        return None;
    };
    // SAFETY: winit's AppKit handle points at the window's live content view,
    // and this runs on the main thread that owns it.
    let view = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
    view.window()
}

/// Order the child in and attach it above `parent`, so it moves with it.
pub(super) fn show_child(parent: &NSWindow, child: &NSWindow) {
    child.orderFront(None);
    // SAFETY: both windows are alive and used on the main thread.
    unsafe { parent.addChildWindow_ordered(child, NSWindowOrderingMode::NSWindowAbove) };
}

/// Detach the child from `parent` and order it out.
pub(super) fn hide_child(parent: &NSWindow, child: &NSWindow) {
    // SAFETY: both windows are alive and used on the main thread.
    unsafe { parent.removeChildWindow(child) };
    child.orderOut(None);
}
