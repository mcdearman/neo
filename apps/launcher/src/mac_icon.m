// Reads an app's icon as RGBA pixels. Called from apps.rs.

#import <AppKit/AppKit.h>

// Draws the icon of the file at `path` into `rgba`, a side × side square of
// straight-alpha RGBA pixels. Returns 1 on success.
int neo_app_icon(const char *path, int side, uint8_t *rgba) {
    @autoreleasepool {
        NSImage *icon = [[NSWorkspace sharedWorkspace] iconForFile:@(path)];
        if (icon == nil || side <= 0) {
            return 0;
        }
        CGColorSpaceRef space = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
        CGContextRef bitmap = CGBitmapContextCreate(rgba, side, side, 8, side * 4, space, kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big);
        CGColorSpaceRelease(space);
        if (bitmap == NULL) {
            return 0;
        }
        memset(rgba, 0, (size_t)side * side * 4);
        NSRect rect = NSMakeRect(0, 0, side, side);
        // Ask for the representation that suits this size, then draw it.
        CGImageRef image = [icon CGImageForProposedRect:&rect context:nil hints:nil];
        if (image != NULL) {
            CGContextSetInterpolationQuality(bitmap, kCGInterpolationHigh);
            CGContextDrawImage(bitmap, CGRectMake(0, 0, side, side), image);
        }
        CGContextRelease(bitmap);
        if (image == NULL) {
            return 0;
        }
        // Core Graphics draws premultiplied; Neo wants straight alpha.
        for (int i = 0; i < side * side; i++) {
            uint8_t *p = rgba + i * 4;
            uint8_t a = p[3];
            if (a != 0 && a != 255) {
                p[0] = (uint8_t)MIN(255, (p[0] * 255 + a / 2) / a);
                p[1] = (uint8_t)MIN(255, (p[1] * 255 + a / 2) / a);
                p[2] = (uint8_t)MIN(255, (p[2] * 255 + a / 2) / a);
            }
        }
        return 1;
    }
}
