// Plays a video with AVFoundation and hands its frames to Rust as RGBA
// pixels. AVPlayer plays the sound itself. Called from player.rs, always on
// the main thread.

#import <AVFoundation/AVFoundation.h>
#import <Accelerate/Accelerate.h>
#import <CoreVideo/CoreVideo.h>
#import <QuartzCore/QuartzCore.h>

@interface NeoPlayer : NSObject
@property(nonatomic, strong) AVPlayer *player;
@property(nonatomic, strong) AVPlayerItem *item;
@property(nonatomic, strong) AVPlayerItemVideoOutput *output;
// A frame fetched by neo_player_next_frame and not yet copied out.
@property(nonatomic, assign) CVPixelBufferRef pending;
@property(nonatomic, assign) int turns;
@end

@implementation NeoPlayer
- (void)dealloc {
    if (_pending != NULL) {
        CVPixelBufferRelease(_pending);
    }
}
@end

// Opens `path`. Frames wider than `max_width` are scaled down as they are
// decoded, which keeps 4K video affordable to copy and upload.
void *neo_player_open(const char *path, int max_width) {
    @autoreleasepool {
        NeoPlayer *p = [NeoPlayer new];
        AVURLAsset *asset = [AVURLAsset URLAssetWithURL:[NSURL fileURLWithPath:@(path)] options:nil];
        NSMutableDictionary *attributes = [@{(id)kCVPixelBufferPixelFormatTypeKey : @(kCVPixelFormatType_32BGRA)} mutableCopy];
        AVAssetTrack *track = [asset tracksWithMediaType:AVMediaTypeVideo].firstObject;
        if (track != nil) {
            CGSize size = track.naturalSize;
            // Phones record sideways and store a rotation. The video output
            // does not apply it, so report the quarter turns for drawing.
            CGAffineTransform t = track.preferredTransform;
            int degrees = (int)lround(atan2(t.b, t.a) * 180.0 / M_PI);
            p.turns = ((degrees % 360 + 360) % 360) / 90;
            if (size.width > max_width && size.width > 0) {
                double scale = max_width / size.width;
                attributes[(id)kCVPixelBufferWidthKey] = @((int)(size.width * scale) & ~1);
                attributes[(id)kCVPixelBufferHeightKey] = @((int)(size.height * scale) & ~1);
            }
        }
        p.item = [AVPlayerItem playerItemWithAsset:asset];
        p.output = [[AVPlayerItemVideoOutput alloc] initWithPixelBufferAttributes:attributes];
        [p.item addOutput:p.output];
        p.player = [AVPlayer playerWithPlayerItem:p.item];
        p.player.actionAtItemEnd = AVPlayerActionAtItemEndPause;
        return (__bridge_retained void *)p;
    }
}

void neo_player_close(void *handle) {
    @autoreleasepool {
        NeoPlayer *p = (__bridge_transfer NeoPlayer *)handle;
        [p.player pause];
        [p.item removeOutput:p.output];
    }
}

// 0 while loading, 1 once it can play, 2 if it cannot be played.
int neo_player_status(void *handle) {
    NeoPlayer *p = (__bridge NeoPlayer *)handle;
    switch (p.item.status) {
    case AVPlayerItemStatusReadyToPlay:
        return 1;
    case AVPlayerItemStatusFailed:
        return 2;
    default:
        return 0;
    }
}

void neo_player_error(void *handle, char *out, int len) {
    NeoPlayer *p = (__bridge NeoPlayer *)handle;
    NSString *message = p.item.error.localizedDescription ?: @"This file could not be played.";
    if (out != NULL && len > 0) {
        strlcpy(out, message.UTF8String ?: "", (size_t)len);
    }
}

void neo_player_set_playing(void *handle, int playing) {
    NeoPlayer *p = (__bridge NeoPlayer *)handle;
    if (playing) {
        [p.player play];
    } else {
        [p.player pause];
    }
}

int neo_player_playing(void *handle) {
    NeoPlayer *p = (__bridge NeoPlayer *)handle;
    return p.player.rate != 0;
}

void neo_player_seek(void *handle, double seconds) {
    NeoPlayer *p = (__bridge NeoPlayer *)handle;
    // Exact seeking, so the picture matches where the slider was let go.
    [p.player seekToTime:CMTimeMakeWithSeconds(seconds, 600) toleranceBefore:kCMTimeZero toleranceAfter:kCMTimeZero];
}

double neo_player_time(void *handle) {
    NeoPlayer *p = (__bridge NeoPlayer *)handle;
    double t = CMTimeGetSeconds(p.player.currentTime);
    return isfinite(t) ? t : 0;
}

// The length in seconds, or a negative number while it is not known.
double neo_player_duration(void *handle) {
    NeoPlayer *p = (__bridge NeoPlayer *)handle;
    double d = CMTimeGetSeconds(p.item.duration);
    return isfinite(d) && d > 0 ? d : -1;
}

void neo_player_set_volume(void *handle, float volume) {
    NeoPlayer *p = (__bridge NeoPlayer *)handle;
    p.player.volume = volume;
}

int neo_player_turns(void *handle) {
    NeoPlayer *p = (__bridge NeoPlayer *)handle;
    return p.turns;
}

// Fetches the frame for this moment if there is a new one, and reports its
// size. Follow with neo_player_copy_frame.
int neo_player_next_frame(void *handle, int *width, int *height) {
    NeoPlayer *p = (__bridge NeoPlayer *)handle;
    CMTime now = [p.output itemTimeForHostTime:CACurrentMediaTime()];
    if (![p.output hasNewPixelBufferForItemTime:now]) {
        return 0;
    }
    CVPixelBufferRef buffer = [p.output copyPixelBufferForItemTime:now itemTimeForDisplay:NULL];
    if (buffer == NULL) {
        return 0;
    }
    if (p.pending != NULL) {
        CVPixelBufferRelease(p.pending);
    }
    p.pending = buffer;
    *width = (int)CVPixelBufferGetWidth(buffer);
    *height = (int)CVPixelBufferGetHeight(buffer);
    return 1;
}

// Copies the fetched frame into `rgba`, which must hold width × height × 4 bytes.
void neo_player_copy_frame(void *handle, uint8_t *rgba) {
    NeoPlayer *p = (__bridge NeoPlayer *)handle;
    CVPixelBufferRef buffer = p.pending;
    if (buffer == NULL) {
        return;
    }
    CVPixelBufferLockBaseAddress(buffer, kCVPixelBufferLock_ReadOnly);
    size_t width = CVPixelBufferGetWidth(buffer), height = CVPixelBufferGetHeight(buffer);
    vImage_Buffer src = {CVPixelBufferGetBaseAddress(buffer), height, width, CVPixelBufferGetBytesPerRow(buffer)};
    vImage_Buffer dst = {rgba, height, width, width * 4};
    // BGRA to RGBA.
    const uint8_t map[4] = {2, 1, 0, 3};
    vImagePermuteChannels_ARGB8888(&src, &dst, map, kvImageNoFlags);
    CVPixelBufferUnlockBaseAddress(buffer, kCVPixelBufferLock_ReadOnly);
    CVPixelBufferRelease(buffer);
    p.pending = NULL;
}

// Runs the main run loop briefly. A windowed app does this all the time;
// headless screenshots have to ask, or AVFoundation never reports progress.
void neo_player_pump(double seconds) {
    [[NSRunLoop mainRunLoop] runUntilDate:[NSDate dateWithTimeIntervalSinceNow:seconds]];
}
