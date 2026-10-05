// Decodes a picture with ImageIO, which reads everything macOS can show,
// including HEIC from iPhones. Called from Rust; see decode.rs.

#import <CoreGraphics/CoreGraphics.h>
#import <Foundation/Foundation.h>
#import <ImageIO/ImageIO.h>

// Returns straight-alpha RGBA pixels, already turned the right way up, or
// NULL. The caller frees them with neo_decode_free.
uint8_t *neo_decode_image(const char *path, int max_side, int *width, int *height) {
    @autoreleasepool {
        NSURL *url = [NSURL fileURLWithPath:@(path)];
        CGImageSourceRef source = CGImageSourceCreateWithURL((__bridge CFURLRef)url, NULL);
        if (source == NULL) {
            return NULL;
        }
        // Asking for a "thumbnail" at full size is how ImageIO applies the
        // camera's orientation for us.
        NSDictionary *options = @{
            (id)kCGImageSourceCreateThumbnailFromImageAlways : @YES,
            (id)kCGImageSourceCreateThumbnailWithTransform : @YES,
            (id)kCGImageSourceThumbnailMaxPixelSize : @(max_side),
        };
        CGImageRef image = CGImageSourceCreateThumbnailAtIndex(source, 0, (__bridge CFDictionaryRef)options);
        CFRelease(source);
        if (image == NULL) {
            return NULL;
        }
        size_t w = CGImageGetWidth(image), h = CGImageGetHeight(image);
        uint8_t *pixels = calloc(w * h, 4);
        CGColorSpaceRef space = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
        CGContextRef bitmap = pixels == NULL ? NULL : CGBitmapContextCreate(pixels, w, h, 8, w * 4, space, kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big);
        CGColorSpaceRelease(space);
        if (bitmap == NULL) {
            free(pixels);
            CGImageRelease(image);
            return NULL;
        }
        CGContextDrawImage(bitmap, CGRectMake(0, 0, w, h), image);
        CGContextRelease(bitmap);
        CGImageRelease(image);
        // Core Graphics draws premultiplied; Neo wants straight alpha.
        for (size_t i = 0; i < w * h; i++) {
            uint8_t *p = pixels + i * 4;
            uint8_t a = p[3];
            if (a != 0 && a != 255) {
                p[0] = (uint8_t)MIN(255, (p[0] * 255 + a / 2) / a);
                p[1] = (uint8_t)MIN(255, (p[1] * 255 + a / 2) / a);
                p[2] = (uint8_t)MIN(255, (p[2] * 255 + a / 2) / a);
            }
        }
        *width = (int)w;
        *height = (int)h;
        return pixels;
    }
}

void neo_decode_free(uint8_t *pixels) {
    free(pixels);
}
