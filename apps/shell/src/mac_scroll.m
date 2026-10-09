// Turning round the scrolling of the trackpad or the mouse, for every
// app. Called from scroll.rs.
//
// macOS scrolls both one way. To have them differ, the scroll events of
// one are turned round as they pass, which the system allows an app it
// has been told may control the computer (Privacy & Security,
// Accessibility), as it does the moving of windows.

#import <ApplicationServices/ApplicationServices.h>
#import <Foundation/Foundation.h>
#include <stdatomic.h>

static atomic_int neo_turn_trackpad = 0;
static atomic_int neo_turn_mouse = 0;
static CFMachPortRef neo_tap = NULL;

static CGEventRef neo_scroll_seen(CGEventTapProxy proxy, CGEventType type, CGEventRef event, void *info) {
    // The system stops a tap that is slow, or when the user types a password: start it again.
    if (type == kCGEventTapDisabledByTimeout || type == kCGEventTapDisabledByUserInput) {
        if (neo_tap != NULL) {
            CGEventTapEnable(neo_tap, true);
        }
        return event;
    }
    if (type != kCGEventScrollWheel) {
        return event;
    }
    // A trackpad scrolls smoothly, by points; a wheel by lines.
    bool trackpad = CGEventGetIntegerValueField(event, kCGScrollWheelEventIsContinuous) != 0;
    if (!atomic_load(trackpad ? &neo_turn_trackpad : &neo_turn_mouse)) {
        return event;
    }
    // Up and down only: sideways is the same whichever way it is held.
    CGEventSetIntegerValueField(event, kCGScrollWheelEventDeltaAxis1, -CGEventGetIntegerValueField(event, kCGScrollWheelEventDeltaAxis1));
    CGEventSetDoubleValueField(event, kCGScrollWheelEventFixedPtDeltaAxis1, -CGEventGetDoubleValueField(event, kCGScrollWheelEventFixedPtDeltaAxis1));
    CGEventSetIntegerValueField(event, kCGScrollWheelEventPointDeltaAxis1, -CGEventGetIntegerValueField(event, kCGScrollWheelEventPointDeltaAxis1));
    return event;
}

// Which of the two to turn round from now on.
void neo_scroll_set(int trackpad, int mouse) {
    atomic_store(&neo_turn_trackpad, trackpad != 0);
    atomic_store(&neo_turn_mouse, mouse != 0);
}

// Watches scroll events on this thread until the app ends. Returns 0 at
// once if the system will not allow it, and does not return otherwise.
int neo_scroll_run(void) {
    @autoreleasepool {
        neo_tap = CGEventTapCreate(kCGSessionEventTap, kCGHeadInsertEventTap, kCGEventTapOptionDefault, CGEventMaskBit(kCGEventScrollWheel), neo_scroll_seen, NULL);
        if (neo_tap == NULL) {
            return 0;
        }
        CFRunLoopSourceRef source = CFMachPortCreateRunLoopSource(kCFAllocatorDefault, neo_tap, 0);
        CFRunLoopAddSource(CFRunLoopGetCurrent(), source, kCFRunLoopCommonModes);
        CGEventTapEnable(neo_tap, true);
        CFRunLoopRun();
        CFRelease(source);
        return 1;
    }
}

// Whether the tap is there and has not been stopped by the system.
int neo_scroll_working(void) {
    return neo_tap != NULL && CGEventTapIsEnabled(neo_tap) ? 1 : 0;
}
