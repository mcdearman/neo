// Finding and moving other apps' windows, for tiling. Called from
// windows.rs.
//
// macOS lets one app move another's windows only through the
// accessibility interface, and only once the user has allowed it in
// System Settings, under Privacy & Security, Accessibility.

#import <AppKit/AppKit.h>
#import <ApplicationServices/ApplicationServices.h>

// The window number of an accessibility window. Not in the headers, but
// what every window manager for the Mac uses to tell which window is which.
extern AXError _AXUIElementGetWindow(AXUIElementRef element, CGWindowID *window);

// Whether this app may move other apps' windows. With `ask` not 0 and
// leave not yet given, the system shows its own request to the user.
int neo_wm_allowed(int ask) {
    // An app that has hung is not waited on for long: the rest are laid out without it.
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        AXUIElementRef everything = AXUIElementCreateSystemWide();
        AXUIElementSetMessagingTimeout(everything, 1.0);
        CFRelease(everything);
    });
    NSDictionary *options = @{(__bridge NSString *)kAXTrustedCheckOptionPrompt : ask ? @YES : @NO};
    return AXIsProcessTrustedWithOptions((__bridge CFDictionaryRef)options) ? 1 : 0;
}

// The height of the screen the system measures from, for turning its
// measure, up from the bottom, into ours, down from the top.
static CGFloat neo_primary_height(void) {
    NSScreen *primary = NSScreen.screens.firstObject;
    return primary != nil ? primary.frame.size.height : 0;
}

// Writes a line for each screen into `out`: the part of it windows may
// use (clear of the menu bar and the Dock) as x, y, width and height,
// measured down from the top left of the main screen. Returns how many.
int neo_wm_screens(char *out, int length) {
    @autoreleasepool {
        NSMutableString *lines = [NSMutableString string];
        CGFloat height = neo_primary_height();
        for (NSScreen *screen in NSScreen.screens) {
            NSRect usable = screen.visibleFrame;
            [lines appendFormat:@"%.0f\t%.0f\t%.0f\t%.0f\n", usable.origin.x, height - usable.origin.y - usable.size.height, usable.size.width, usable.size.height];
        }
        const char *text = lines.UTF8String;
        if (out == NULL || text == NULL || strlen(text) + 1 > (size_t)length) {
            return 0;
        }
        strlcpy(out, text, (size_t)length);
        return (int)NSScreen.screens.count;
    }
}

// The accessibility window of process `pid` whose window number is
// `wanted`, or NULL. The caller releases it.
static AXUIElementRef neo_window(pid_t pid, CGWindowID wanted) {
    AXUIElementRef app = AXUIElementCreateApplication(pid);
    if (app == NULL) {
        return NULL;
    }
    CFArrayRef windows = NULL;
    AXUIElementRef found = NULL;
    if (AXUIElementCopyAttributeValue(app, kAXWindowsAttribute, (CFTypeRef *)&windows) == kAXErrorSuccess && windows != NULL) {
        for (CFIndex i = 0; i < CFArrayGetCount(windows); i++) {
            AXUIElementRef window = CFArrayGetValueAtIndex(windows, i);
            CGWindowID number = 0;
            if (_AXUIElementGetWindow(window, &number) == kAXErrorSuccess && number == wanted) {
                found = CFRetain(window);
                break;
            }
        }
        CFRelease(windows);
    }
    CFRelease(app);
    return found;
}

static BOOL neo_flag(AXUIElementRef window, CFStringRef attribute) {
    CFTypeRef value = NULL;
    BOOL set = NO;
    if (AXUIElementCopyAttributeValue(window, attribute, &value) == kAXErrorSuccess && value != NULL) {
        set = CFGetTypeID(value) == CFBooleanGetTypeID() && CFBooleanGetValue(value);
        CFRelease(value);
    }
    return set;
}

// Whether a window is one to tile: an ordinary window, not a dialog or a
// panel, that is neither minimised nor filling the screen on its own.
static BOOL neo_tileable(AXUIElementRef window) {
    CFTypeRef subrole = NULL;
    BOOL standard = NO;
    if (AXUIElementCopyAttributeValue(window, kAXSubroleAttribute, &subrole) == kAXErrorSuccess && subrole != NULL) {
        standard = CFGetTypeID(subrole) == CFStringGetTypeID() && CFEqual(subrole, kAXStandardWindowSubrole);
        CFRelease(subrole);
    }
    return standard && !neo_flag(window, kAXMinimizedAttribute) && !neo_flag(window, CFSTR("AXFullScreen"));
}

// Writes a line for each window on screen that can be tiled into `out`,
// the frontmost first: its number, its process, its x, y, width and
// height measured down from the top left of the main screen, and its
// app's name, with tabs between. Returns how many. Nothing, without leave.
int neo_wm_windows(char *out, int length) {
    @autoreleasepool {
        if (out == NULL || length <= 0) {
            return 0;
        }
        out[0] = 0;
        if (!AXIsProcessTrusted()) {
            return 0;
        }
        NSArray *all = CFBridgingRelease(CGWindowListCopyWindowInfo(kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements, kCGNullWindowID));
        NSMutableString *lines = [NSMutableString string];
        pid_t me = NSProcessInfo.processInfo.processIdentifier;
        int written = 0;
        for (NSDictionary *info in all) {
            // Ordinary windows are on layer 0; menus, the Dock and panels kept on top are not.
            if ([info[(__bridge NSString *)kCGWindowLayer] intValue] != 0 || [info[(__bridge NSString *)kCGWindowAlpha] doubleValue] <= 0) {
                continue;
            }
            pid_t pid = [info[(__bridge NSString *)kCGWindowOwnerPID] intValue];
            CGWindowID number = [info[(__bridge NSString *)kCGWindowNumber] unsignedIntValue];
            if (pid == me) {
                continue;
            }
            AXUIElementRef window = neo_window(pid, number);
            if (window == NULL) {
                continue;
            }
            BOOL tile = neo_tileable(window);
            CFRelease(window);
            if (!tile) {
                continue;
            }
            CGRect bounds = CGRectZero;
            CGRectMakeWithDictionaryRepresentation((__bridge CFDictionaryRef)info[(__bridge NSString *)kCGWindowBounds], &bounds);
            NSString *app = info[(__bridge NSString *)kCGWindowOwnerName] ?: @"";
            app = [[app stringByReplacingOccurrencesOfString:@"\t" withString:@" "] stringByReplacingOccurrencesOfString:@"\n" withString:@" "];
            [lines appendFormat:@"%u\t%d\t%.0f\t%.0f\t%.0f\t%.0f\t%@\n", number, pid, bounds.origin.x, bounds.origin.y, bounds.size.width, bounds.size.height, app];
            written++;
        }
        const char *text = lines.UTF8String;
        if (text == NULL || strlen(text) + 1 > (size_t)length) {
            return 0;
        }
        strlcpy(out, text, (size_t)length);
        return written;
    }
}

// Moves a window and sizes it, measured down from the top left of the
// main screen. Returns 1 if the window was found and told.
int neo_wm_place(int pid, unsigned int number, double x, double y, double w, double h) {
    AXUIElementRef window = neo_window(pid, number);
    if (window == NULL) {
        return 0;
    }
    CGPoint at = CGPointMake(x, y);
    CGSize size = CGSizeMake(w, h);
    AXValueRef where = AXValueCreate(kAXValueTypeCGPoint, &at);
    AXValueRef how_big = AXValueCreate(kAXValueTypeCGSize, &size);
    // Size, place, and size again: a window being moved to a smaller
    // screen, or held to the edge of this one, takes its size only once it is there.
    AXUIElementSetAttributeValue(window, kAXSizeAttribute, how_big);
    AXError moved = AXUIElementSetAttributeValue(window, kAXPositionAttribute, where);
    AXUIElementSetAttributeValue(window, kAXSizeAttribute, how_big);
    CFRelease(where);
    CFRelease(how_big);
    CFRelease(window);
    return moved == kAXErrorSuccess ? 1 : 0;
}

// Brings a window to the front and gives it the keyboard.
int neo_wm_focus(int pid, unsigned int number) {
    AXUIElementRef window = neo_window(pid, number);
    if (window == NULL) {
        return 0;
    }
    AXUIElementPerformAction(window, kAXRaiseAction);
    AXUIElementSetAttributeValue(window, kAXMainAttribute, kCFBooleanTrue);
    CFRelease(window);
    [[NSRunningApplication runningApplicationWithProcessIdentifier:pid] activateWithOptions:0];
    return 1;
}

// The number of the window that has the keyboard, or 0.
unsigned int neo_wm_focused(void) {
    AXUIElementRef system = AXUIElementCreateSystemWide();
    AXUIElementRef app = NULL;
    CGWindowID number = 0;
    if (AXUIElementCopyAttributeValue(system, kAXFocusedApplicationAttribute, (CFTypeRef *)&app) == kAXErrorSuccess && app != NULL) {
        AXUIElementRef window = NULL;
        if (AXUIElementCopyAttributeValue(app, kAXFocusedWindowAttribute, (CFTypeRef *)&window) == kAXErrorSuccess && window != NULL) {
            _AXUIElementGetWindow(window, &number);
            CFRelease(window);
        }
        CFRelease(app);
    }
    CFRelease(system);
    return number;
}

// Whether the mouse's button is down: a window may be being dragged.
int neo_wm_button_down(void) {
    return CGEventSourceButtonState(kCGEventSourceStateCombinedSessionState, kCGMouseButtonLeft) ? 1 : 0;
}
