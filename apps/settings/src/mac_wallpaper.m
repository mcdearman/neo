// Reads and sets the desktop picture. Called from wallpaper.rs.

#import <AppKit/AppKit.h>

// Sets the picture at `path` as the desktop picture of every screen,
// keeping how each screen fits its picture. Returns 1 if every screen
// took it. To be called on the main thread.
int neo_set_wallpaper(const char *path) {
    @autoreleasepool {
        NSURL *url = [NSURL fileURLWithPath:@(path)];
        NSWorkspace *workspace = [NSWorkspace sharedWorkspace];
        BOOL all = NSScreen.screens.count > 0;
        for (NSScreen *screen in NSScreen.screens) {
            NSDictionary *options = [workspace desktopImageOptionsForScreen:screen] ?: @{};
            NSError *error = nil;
            if (![workspace setDesktopImageURL:url forScreen:screen options:options error:&error]) {
                all = NO;
            }
        }
        return all ? 1 : 0;
    }
}

// Writes the path of the main screen's desktop picture into `out`, which
// holds `length` bytes. Returns 1 if there is one and it fitted.
int neo_get_wallpaper(char *out, int length) {
    @autoreleasepool {
        NSScreen *screen = NSScreen.mainScreen;
        if (screen == nil || out == NULL || length <= 0) {
            return 0;
        }
        NSURL *url = [[NSWorkspace sharedWorkspace] desktopImageURLForScreen:screen];
        const char *path = url.fileSystemRepresentation;
        if (path == NULL || strlen(path) + 1 > (size_t)length) {
            return 0;
        }
        strlcpy(out, path, (size_t)length);
        return 1;
    }
}
