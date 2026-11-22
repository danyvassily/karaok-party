#import <AVFoundation/AVFoundation.h>
#import <CoreAudio/CoreAudio.h>
#import <CoreMedia/CoreMedia.h>
#import <Foundation/Foundation.h>
#import <IOKit/IOKitLib.h>

#include <math.h>
#include <stdatomic.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

typedef struct {
    char name[256];
    uint32_t sample_rate;
    uint16_t channels;
    bool is_default;
    bool is_iphone;
} KpAvDeviceInfo;

typedef void (*KpAvAudioCallback)(
    const float *samples,
    size_t frames,
    uint32_t sample_rate,
    uint16_t channels,
    void *user_ctx
);

static NSArray<AVCaptureDevice *> *kp_discover_audio_devices(void) {
    if (@available(macOS 14.0, *)) {
        NSArray<AVCaptureDeviceType> *types = @[
            AVCaptureDeviceTypeMicrophone,
            AVCaptureDeviceTypeExternal
        ];
        AVCaptureDeviceDiscoverySession *session =
            [AVCaptureDeviceDiscoverySession discoverySessionWithDeviceTypes:types
                                                                   mediaType:AVMediaTypeAudio
                                                                    position:AVCaptureDevicePositionUnspecified];
        return session.devices ?: @[];
    }
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
    return [AVCaptureDevice devicesWithMediaType:AVMediaTypeAudio] ?: @[];
#pragma clang diagnostic pop
}

bool kp_av_is_clamshell_closed(void) {
    CFMutableDictionaryRef matching = IOServiceMatching("IOPMrootDomain");
    if (!matching) {
        return false;
    }
    mach_port_t port = MACH_PORT_NULL;
    if (@available(macOS 12.0, *)) {
        port = kIOMainPortDefault;
    }
    io_service_t service = IOServiceGetMatchingService(port, matching);
    if (service == IO_OBJECT_NULL) {
        return false;
    }
    bool closed = false;
    CFTypeRef prop = IORegistryEntryCreateCFProperty(
        service,
        CFSTR("AppleClamshellState"),
        kCFAllocatorDefault,
        0
    );
    if (prop) {
        if (CFGetTypeID(prop) == CFBooleanGetTypeID()) {
            closed = CFBooleanGetValue((CFBooleanRef)prop);
        }
        CFRelease(prop);
    }
    IOObjectRelease(service);
    return closed;
}

int32_t kp_av_mic_auth_status(bool request_if_needed) {
    AVAuthorizationStatus status =
        [AVCaptureDevice authorizationStatusForMediaType:AVMediaTypeAudio];
    if (status == AVAuthorizationStatusNotDetermined && request_if_needed) {
        if ([NSThread isMainThread]) {
            // Never block the AppKit main runloop waiting for user dialog interaction
            [AVCaptureDevice requestAccessForMediaType:AVMediaTypeAudio
                                     completionHandler:^(BOOL granted) {
                                         (void)granted;
                                     }];
        } else {
            dispatch_semaphore_t sem = dispatch_semaphore_create(0);
            [AVCaptureDevice requestAccessForMediaType:AVMediaTypeAudio
                                     completionHandler:^(BOOL granted) {
                                         (void)granted;
                                         dispatch_semaphore_signal(sem);
                                     }];
            dispatch_semaphore_wait(
                sem,
                dispatch_time(DISPATCH_TIME_NOW, (int64_t)(4 * NSEC_PER_SEC))
            );
            status = [AVCaptureDevice authorizationStatusForMediaType:AVMediaTypeAudio];
        }
    }
    return (int32_t)status;
}

size_t kp_av_list_input_devices(KpAvDeviceInfo *out_buf, size_t max_count) {
    if (!out_buf || max_count == 0) {
        return 0;
    }
    @autoreleasepool {
        AVCaptureDevice *defaultDev =
            [AVCaptureDevice defaultDeviceWithMediaType:AVMediaTypeAudio];
        NSString *defaultUid = defaultDev.uniqueID;
        NSArray<AVCaptureDevice *> *devices = kp_discover_audio_devices();

        size_t written = 0;
        for (AVCaptureDevice *dev in devices) {
            if (written >= max_count) {
                break;
            }
            NSString *name = dev.localizedName ?: @"Audio Input";
            const char *utf8Name = name.UTF8String;
            if (!utf8Name || utf8Name[0] == '\0') {
                continue;
            }

            uint32_t sample_rate = 48000;
            uint16_t channels = 1;
            AVCaptureDeviceFormat *activeFmt = dev.activeFormat;
            if (activeFmt && activeFmt.formatDescription) {
                const AudioStreamBasicDescription *asbd =
                    CMAudioFormatDescriptionGetStreamBasicDescription(activeFmt.formatDescription);
                if (asbd) {
                    if (asbd->mSampleRate >= 8000.0) {
                        sample_rate = (uint32_t)llround(asbd->mSampleRate);
                    }
                    if (asbd->mChannelsPerFrame >= 1) {
                        channels = (uint16_t)asbd->mChannelsPerFrame;
                    }
                }
            }

            NSString *lowerName = name.lowercaseString;
            NSString *lowerModel = (dev.modelID ?: @"").lowercaseString;
            bool is_iphone =
                [lowerName containsString:@"iphone"] ||
                [lowerName containsString:@"continuity"] ||
                [lowerModel containsString:@"iphone"];
            bool is_default =
                defaultUid != nil && [dev.uniqueID isEqualToString:defaultUid];

            KpAvDeviceInfo info;
            memset(&info, 0, sizeof(info));
            strlcpy(info.name, utf8Name, sizeof(info.name));
            info.sample_rate = sample_rate;
            info.channels = channels;
            info.is_default = is_default;
            info.is_iphone = is_iphone;
            out_buf[written++] = info;
        }
        return written;
    }
}

#define KP_AV_SCRATCH_FLOATS 16384

@interface KpAvCaptureDelegate : NSObject <AVCaptureAudioDataOutputSampleBufferDelegate> {
@public
    _Atomic(bool) _active;
    KpAvAudioCallback _callback;
    void *_user_ctx;
    float _scratch[KP_AV_SCRATCH_FLOATS];
}
@property (nonatomic, strong) AVCaptureSession *session;
@property (nonatomic, strong) AVCaptureAudioDataOutput *output;
@property (nonatomic, strong) dispatch_queue_t queue;
@end

@implementation KpAvCaptureDelegate

- (void)captureOutput:(AVCaptureOutput *)output
didOutputSampleBuffer:(CMSampleBufferRef)sampleBuffer
       fromConnection:(AVCaptureConnection *)connection {
    (void)output;
    (void)connection;
    if (!atomic_load_explicit(&_active, memory_order_acquire) || !_callback || !sampleBuffer) {
        return;
    }

    CMFormatDescriptionRef fmt = CMSampleBufferGetFormatDescription(sampleBuffer);
    if (!fmt) {
        return;
    }
    const AudioStreamBasicDescription *asbd =
        CMAudioFormatDescriptionGetStreamBasicDescription(fmt);
    if (!asbd ||
        asbd->mFormatID != kAudioFormatLinearPCM ||
        (asbd->mFormatFlags & kAudioFormatFlagIsFloat) == 0 ||
        (asbd->mFormatFlags & kAudioFormatFlagIsNonInterleaved) != 0 ||
        asbd->mBitsPerChannel != 32 ||
        asbd->mChannelsPerFrame == 0 ||
        asbd->mChannelsPerFrame > 32) {
        return;
    }
    uint32_t sr = asbd->mSampleRate >= 8000.0 ? (uint32_t)llround(asbd->mSampleRate) : 48000;
    uint16_t ch = (uint16_t)asbd->mChannelsPerFrame;

    CMBlockBufferRef block = CMSampleBufferGetDataBuffer(sampleBuffer);
    if (!block) {
        return;
    }

    size_t lengthAtOffset = 0;
    size_t totalLength = 0;
    char *dataPtr = NULL;
    OSStatus status = CMBlockBufferGetDataPointer(
        block,
        0,
        &lengthAtOffset,
        &totalLength,
        &dataPtr
    );
    if (status != kCMBlockBufferNoErr || totalLength == 0) {
        return;
    }

    const size_t bytesPerFrame = sizeof(float) * (size_t)ch;
    if (bytesPerFrame == 0) {
        return;
    }

    const float *samples = NULL;
    size_t frames = 0;
    if (dataPtr != NULL &&
        lengthAtOffset == totalLength &&
        (((uintptr_t)dataPtr) % _Alignof(float)) == 0) {
        samples = (const float *)dataPtr;
        frames = totalLength / bytesPerFrame;
    } else {
        size_t maxBytes = sizeof(_scratch);
        size_t copyBytes = totalLength < maxBytes ? totalLength : maxBytes;
        copyBytes -= (copyBytes % bytesPerFrame);
        if (copyBytes == 0) {
            return;
        }
        if (CMBlockBufferCopyDataBytes(block, 0, copyBytes, _scratch) != kCMBlockBufferNoErr) {
            return;
        }
        samples = _scratch;
        frames = copyBytes / bytesPerFrame;
    }

    if (frames > 0 && atomic_load_explicit(&_active, memory_order_acquire)) {
        _callback(samples, frames, sr, ch, _user_ctx);
    }
}

@end

void *kp_av_start_capture(
    const char *target_name,
    KpAvAudioCallback callback,
    void *user_ctx,
    uint32_t *out_sample_rate,
    uint16_t *out_channels,
    char *err_buf,
    size_t err_buf_len
) {
    if (!target_name || !callback) {
        if (err_buf && err_buf_len > 0) {
            strlcpy(err_buf, "Paramètres de capture invalides.", err_buf_len);
        }
        return NULL;
    }

    @autoreleasepool {
        NSString *wanted = [NSString stringWithUTF8String:target_name];
        AVCaptureDevice *found = nil;
        for (AVCaptureDevice *dev in kp_discover_audio_devices()) {
            if ([dev.localizedName isEqualToString:wanted]) {
                found = dev;
                break;
            }
        }
        if (!found) {
            if (err_buf && err_buf_len > 0) {
                snprintf(
                    err_buf,
                    err_buf_len,
                    "Micro introuvable : « %s ». Vérifiez qu'il est connecté au Mac.",
                    target_name
                );
            }
            return NULL;
        }

        NSError *error = nil;
        AVCaptureDeviceInput *input = [AVCaptureDeviceInput deviceInputWithDevice:found error:&error];
        if (!input || error) {
            if (err_buf && err_buf_len > 0) {
                const char *msg = error.localizedDescription.UTF8String ?: "Impossible d'ouvrir l'entrée audio";
                strlcpy(err_buf, msg, err_buf_len);
            }
            return NULL;
        }

        AVCaptureSession *session = [[AVCaptureSession alloc] init];
        if (![session canAddInput:input]) {
            if (err_buf && err_buf_len > 0) {
                strlcpy(err_buf, "AVCaptureSession refuse l'entrée audio sélectionnée.", err_buf_len);
            }
            return NULL;
        }
        [session addInput:input];

        AVCaptureAudioDataOutput *output = [[AVCaptureAudioDataOutput alloc] init];
        output.audioSettings = @{
            AVFormatIDKey: @(kAudioFormatLinearPCM),
            AVLinearPCMBitDepthKey: @(32),
            AVLinearPCMIsFloatKey: @YES,
            AVLinearPCMIsBigEndianKey: @NO,
            AVLinearPCMIsNonInterleaved: @NO,
        };

        if (![session canAddOutput:output]) {
            if (err_buf && err_buf_len > 0) {
                strlcpy(err_buf, "AVCaptureSession refuse la sortie PCM Float32.", err_buf_len);
            }
            return NULL;
        }
        [session addOutput:output];

        dispatch_queue_attr_t attr = dispatch_queue_attr_make_with_qos_class(
            DISPATCH_QUEUE_SERIAL,
            QOS_CLASS_USER_INTERACTIVE,
            0
        );
        dispatch_queue_t queue = dispatch_queue_create("dev.karaokparty.avmic", attr);

        KpAvCaptureDelegate *delegate = [[KpAvCaptureDelegate alloc] init];
        atomic_init(&delegate->_active, true);
        delegate->_callback = callback;
        delegate->_user_ctx = user_ctx;
        delegate.session = session;
        delegate.output = output;
        delegate.queue = queue;

        [output setSampleBufferDelegate:delegate queue:queue];
        [session startRunning];

        if (!session.isRunning) {
            atomic_store_explicit(&delegate->_active, false, memory_order_release);
            [session stopRunning];
            [output setSampleBufferDelegate:nil queue:NULL];
            dispatch_sync(queue, ^{
                delegate->_callback = NULL;
                delegate->_user_ctx = NULL;
            });
            if (err_buf && err_buf_len > 0) {
                strlcpy(err_buf, "Le flux micro AVFoundation n'a pas pu démarrer.", err_buf_len);
            }
            return NULL;
        }

        uint32_t sr = 48000;
        uint16_t ch = 1;
        AVCaptureDeviceFormat *fmt = found.activeFormat;
        if (fmt && fmt.formatDescription) {
            const AudioStreamBasicDescription *asbd =
                CMAudioFormatDescriptionGetStreamBasicDescription(fmt.formatDescription);
            if (asbd) {
                if (asbd->mSampleRate >= 8000.0) {
                    sr = (uint32_t)llround(asbd->mSampleRate);
                }
                if (asbd->mChannelsPerFrame >= 1) {
                    ch = (uint16_t)asbd->mChannelsPerFrame;
                }
            }
        }
        if (out_sample_rate) {
            *out_sample_rate = sr;
        }
        if (out_channels) {
            *out_channels = ch;
        }

        return (void *)CFBridgingRetain(delegate);
    }
}

void kp_av_stop_capture(void *handle) {
    if (!handle) {
        return;
    }
    @autoreleasepool {
        KpAvCaptureDelegate *delegate = (__bridge_transfer KpAvCaptureDelegate *)handle;
        atomic_store_explicit(&delegate->_active, false, memory_order_release);
        [delegate.output setSampleBufferDelegate:nil queue:NULL];
        [delegate.session stopRunning];
        if (delegate.queue) {
            dispatch_sync(delegate.queue, ^{
                delegate->_callback = NULL;
                delegate->_user_ctx = NULL;
            });
        }
    }
}
