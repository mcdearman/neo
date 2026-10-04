// Joins recordings and decodes their frames with AVFoundation, for pausing
// and GIF export on macOS. Called from Rust; see media.rs.

#import <AVFoundation/AVFoundation.h>
#import <CoreGraphics/CoreGraphics.h>

static void set_error(char *err, int len, NSString *message) {
    if (err != NULL && len > 0) {
        strlcpy(err, message.UTF8String ?: "unknown error", (size_t)len);
    }
}

// Writes the movies at `paths`, one after another, to `out` without
// re-encoding. Returns 0 on success.
int neo_media_concat(const char *const *paths, int count, const char *out, char *err, int errlen) {
    @autoreleasepool {
        AVMutableComposition *composition = [AVMutableComposition composition];
        CMTime at = kCMTimeZero;
        for (int i = 0; i < count; i++) {
            NSURL *url = [NSURL fileURLWithPath:@(paths[i])];
            AVURLAsset *asset = [AVURLAsset URLAssetWithURL:url options:@{AVURLAssetPreferPreciseDurationAndTimingKey : @YES}];
            CMTime duration = asset.duration;
            if (!CMTIME_IS_NUMERIC(duration) || CMTIME_COMPARE_INLINE(duration, <=, kCMTimeZero)) {
                continue;
            }
            NSError *error = nil;
            if (![composition insertTimeRange:CMTimeRangeMake(kCMTimeZero, duration) ofAsset:asset atTime:at error:&error]) {
                set_error(err, errlen, error.localizedDescription);
                return 1;
            }
            at = CMTimeAdd(at, duration);
        }
        if (CMTIME_COMPARE_INLINE(at, <=, kCMTimeZero)) {
            set_error(err, errlen, @"the recorded parts are empty");
            return 1;
        }
        NSURL *outURL = [NSURL fileURLWithPath:@(out)];
        [[NSFileManager defaultManager] removeItemAtURL:outURL error:nil];
        AVAssetExportSession *session = [[AVAssetExportSession alloc] initWithAsset:composition presetName:AVAssetExportPresetPassthrough];
        session.outputURL = outURL;
        session.outputFileType = AVFileTypeQuickTimeMovie;
        dispatch_semaphore_t done = dispatch_semaphore_create(0);
        [session exportAsynchronouslyWithCompletionHandler:^{
          dispatch_semaphore_signal(done);
        }];
        dispatch_semaphore_wait(done, DISPATCH_TIME_FOREVER);
        if (session.status != AVAssetExportSessionStatusCompleted) {
            set_error(err, errlen, session.error.localizedDescription ?: @"the export did not finish");
            return 1;
        }
        return 0;
    }
}

// Called once per frame with straight-alpha RGBA pixels. Return non-zero to stop.
typedef int (*neo_frame_fn)(void *context, const uint8_t *rgba, int width, int height);

// Decodes `path` at `fps` frames a second, scaled to fit `max_width`, and
// hands each frame to `on_frame`. Returns 0 on success.
int neo_media_frames(const char *path, double fps, int max_width, neo_frame_fn on_frame, void *context, char *err, int errlen) {
    @autoreleasepool {
        AVURLAsset *asset = [AVURLAsset URLAssetWithURL:[NSURL fileURLWithPath:@(path)] options:@{AVURLAssetPreferPreciseDurationAndTimingKey : @YES}];
        double seconds = CMTimeGetSeconds(asset.duration);
        if (!(seconds > 0)) {
            set_error(err, errlen, @"the recording is empty");
            return 1;
        }
        AVAssetImageGenerator *generator = [[AVAssetImageGenerator alloc] initWithAsset:asset];
        generator.appliesPreferredTrackTransform = YES;
        CMTime tolerance = CMTimeMakeWithSeconds(0.5 / fps, 600);
        generator.requestedTimeToleranceBefore = tolerance;
        generator.requestedTimeToleranceAfter = tolerance;
        generator.maximumSize = CGSizeMake(max_width, max_width * 8);
        CGColorSpaceRef space = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
        int total = (int)ceil(seconds * fps);
        if (total < 1) {
            total = 1;
        }
        int width = 0, height = 0, sent = 0, status = 0;
        NSMutableData *pixels = nil;
        for (int i = 0; i < total && status == 0; i++) {
            @autoreleasepool {
                NSError *error = nil;
                CGImageRef image = [generator copyCGImageAtTime:CMTimeMakeWithSeconds(i / fps, 600) actualTime:NULL error:&error];
                if (image == NULL) {
                    // The last instant can have no frame; an unreadable start is an error.
                    if (sent == 0) {
                        set_error(err, errlen, error.localizedDescription ?: @"could not read the recording");
                        status = 1;
                    }
                    continue;
                }
                if (pixels == nil) {
                    width = (int)CGImageGetWidth(image);
                    height = (int)CGImageGetHeight(image);
                    pixels = [NSMutableData dataWithLength:(NSUInteger)width * (NSUInteger)height * 4];
                }
                // Every frame is drawn at the first frame's size, which a GIF requires.
                CGContextRef bitmap = CGBitmapContextCreate(pixels.mutableBytes, (size_t)width, (size_t)height, 8, (size_t)width * 4, space, kCGImageAlphaNoneSkipLast | kCGBitmapByteOrder32Big);
                if (bitmap != NULL) {
                    CGContextDrawImage(bitmap, CGRectMake(0, 0, width, height), image);
                    CGContextRelease(bitmap);
                    if (on_frame(context, pixels.bytes, width, height) != 0) {
                        set_error(err, errlen, @"could not write the frame");
                        status = 1;
                    }
                    sent++;
                }
                CGImageRelease(image);
            }
        }
        CGColorSpaceRelease(space);
        return status;
    }
}
