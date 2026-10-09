// What Settings asks of macOS itself: the desktop picture (wallpaper.rs),
// the sound devices (sound.rs) and Bluetooth (bluetooth.rs).

// ---- The desktop picture.

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

// ---- Sound: which devices there are, and which are the defaults.

#import <CoreAudio/CoreAudio.h>

static UInt32 neo_channels(AudioDeviceID device, AudioObjectPropertyScope scope) {
    AudioObjectPropertyAddress address = {kAudioDevicePropertyStreamConfiguration, scope, kAudioObjectPropertyElementMain};
    UInt32 size = 0;
    if (AudioObjectGetPropertyDataSize(device, &address, 0, NULL, &size) != noErr || size == 0) {
        return 0;
    }
    AudioBufferList *buffers = malloc(size);
    UInt32 channels = 0;
    if (buffers != NULL && AudioObjectGetPropertyData(device, &address, 0, NULL, &size, buffers) == noErr) {
        for (UInt32 i = 0; i < buffers->mNumberBuffers; i++) {
            channels += buffers->mBuffers[i].mNumberChannels;
        }
    }
    free(buffers);
    return channels;
}

static AudioDeviceID neo_default_device(AudioObjectPropertySelector which) {
    AudioObjectPropertyAddress address = {which, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain};
    AudioDeviceID device = kAudioObjectUnknown;
    UInt32 size = sizeof(device);
    AudioObjectGetPropertyData(kAudioObjectSystemObject, &address, 0, NULL, &size, &device);
    return device;
}

// Writes a line for each sound device into `out`, which holds `length`
// bytes: its number, whether it takes sound in (1 or 0), whether it gives
// sound out, whether it is the default for each, and its name, with tabs
// between. Returns how many devices were written.
int neo_audio_devices(char *out, int length) {
    @autoreleasepool {
        if (out == NULL || length <= 0) {
            return 0;
        }
        out[0] = 0;
        AudioObjectPropertyAddress address = {kAudioHardwarePropertyDevices, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain};
        UInt32 size = 0;
        if (AudioObjectGetPropertyDataSize(kAudioObjectSystemObject, &address, 0, NULL, &size) != noErr || size == 0) {
            return 0;
        }
        UInt32 count = size / sizeof(AudioDeviceID);
        AudioDeviceID *devices = malloc(size);
        if (devices == NULL || AudioObjectGetPropertyData(kAudioObjectSystemObject, &address, 0, NULL, &size, devices) != noErr) {
            free(devices);
            return 0;
        }
        AudioDeviceID default_in = neo_default_device(kAudioHardwarePropertyDefaultInputDevice);
        AudioDeviceID default_out = neo_default_device(kAudioHardwarePropertyDefaultOutputDevice);
        NSMutableString *lines = [NSMutableString string];
        int written = 0;
        for (UInt32 i = 0; i < count; i++) {
            AudioObjectPropertyAddress name_address = {kAudioObjectPropertyName, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain};
            CFStringRef name = NULL;
            UInt32 name_size = sizeof(name);
            if (AudioObjectGetPropertyData(devices[i], &name_address, 0, NULL, &name_size, &name) != noErr || name == NULL) {
                continue;
            }
            BOOL takes = neo_channels(devices[i], kAudioObjectPropertyScopeInput) > 0;
            BOOL gives = neo_channels(devices[i], kAudioObjectPropertyScopeOutput) > 0;
            NSString *plain = [[(__bridge NSString *)name stringByReplacingOccurrencesOfString:@"\t" withString:@" "] stringByReplacingOccurrencesOfString:@"\n" withString:@" "];
            [lines appendFormat:@"%u\t%d\t%d\t%d\t%d\t%@\n", devices[i], takes, gives, devices[i] == default_in, devices[i] == default_out, plain];
            CFRelease(name);
            written++;
        }
        free(devices);
        const char *text = lines.UTF8String;
        if (text == NULL || strlen(text) + 1 > (size_t)length) {
            return 0;
        }
        strlcpy(out, text, (size_t)length);
        return written;
    }
}

// Makes a device the default: for sound in if `input` is not 0, for sound
// out otherwise. Returns 1 if the system took it.
int neo_audio_set_default(unsigned int device, int input) {
    AudioObjectPropertyAddress address = {input ? kAudioHardwarePropertyDefaultInputDevice : kAudioHardwarePropertyDefaultOutputDevice, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain};
    AudioDeviceID id = device;
    return AudioObjectSetPropertyData(kAudioObjectSystemObject, &address, 0, NULL, sizeof(id), &id) == noErr ? 1 : 0;
}

// ---- Bluetooth: turning it on and off.
//
// The system has no public way to do this. These two are what its own
// settings use, and have been there unchanged for many releases.

extern int IOBluetoothPreferenceGetControllerPowerState(void);
extern void IOBluetoothPreferenceSetControllerPowerState(int state);

// Whether Bluetooth is on: 1, or 0.
int neo_bluetooth_power(void) {
    return IOBluetoothPreferenceGetControllerPowerState() ? 1 : 0;
}

void neo_bluetooth_set_power(int on) {
    IOBluetoothPreferenceSetControllerPowerState(on ? 1 : 0);
}
