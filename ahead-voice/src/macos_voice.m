#import <AVFoundation/AVFoundation.h>
#import <AppKit/AppKit.h>
#import <Foundation/Foundation.h>
#include <math.h>
#include <stdint.h>
#include <string.h>

typedef void (*AHEADVoiceEventCallback)(
    void *context,
    int32_t event,
    const uint8_t *audio,
    size_t length,
    uint64_t value,
    const char *error
);
typedef void (*AHEADVoiceReleaseCallback)(const void *context);

@interface AHEADVoiceCapture : NSObject
@property(nonatomic, assign) AHEADVoiceEventCallback callback;
@property(nonatomic, assign) AHEADVoiceReleaseCallback releaseCallback;
@property(nonatomic, assign) void *context;
@property(nonatomic, strong) dispatch_queue_t queue;
@property(nonatomic, strong) AVAudioEngine *engine;
@property(nonatomic, strong) AVAudioConverter *converter;
@property(nonatomic, strong) AVAudioFormat *voiceFormat;
@property(nonatomic, strong) AVAudioPlayerNode *player;
@property(nonatomic, strong) id audioConfigurationObserver;
@property(nonatomic, strong) id systemSleepObserver;
@property(atomic, assign) BOOL stopped;
@property(atomic, assign) BOOL muted;
@property(atomic, assign) BOOL speechActive;
@property(nonatomic, assign) BOOL outputActive;
@property(nonatomic, assign) NSUInteger scheduledBuffers;
@property(nonatomic, assign) NSUInteger scheduledAudioBytes;
@property(nonatomic, assign) uint64_t playbackGeneration;
@property(nonatomic, assign) NSTimeInterval silenceSeconds;
- (void)emit:(int32_t)event audio:(const uint8_t *)audio length:(size_t)length value:(uint64_t)value error:(NSString *)error;
@end

@implementation AHEADVoiceCapture

- (void)emit:(int32_t)event audio:(const uint8_t *)audio length:(size_t)length error:(NSString *)error {
    [self emit:event audio:audio length:length value:0 error:error];
}

- (void)emit:(int32_t)event audio:(const uint8_t *)audio length:(size_t)length value:(uint64_t)value error:(NSString *)error {
    if (self.callback == NULL || self.stopped) {
        return;
    }
    self.callback(
        self.context,
        event,
        audio,
        length,
        value,
        error == nil ? "" : error.UTF8String
    );
}

- (void)start {
    dispatch_async(self.queue, ^{
        if (@available(macOS 10.14, *)) {
            [AVCaptureDevice requestAccessForMediaType:AVMediaTypeAudio
                                     completionHandler:^(BOOL granted) {
                dispatch_async(self.queue, ^{
                    if (self.stopped) {
                        return;
                    }
                    if (!granted) {
                        [self emit:4 audio:NULL length:0 error:@"Microphone access was denied. Enable it in System Settings > Privacy & Security > Microphone."];
                        return;
                    }
                    [self startAudioEngine];
                });
            }];
        } else {
            [self emit:4 audio:NULL length:0 error:@"Voice calls require macOS 10.14 or newer."];
        }
    });
}

- (void)startAudioEngine {
    if (self.stopped) {
        return;
    }
    AVAudioEngine *engine = [[AVAudioEngine alloc] init];
    AVAudioInputNode *input = engine.inputNode;
    NSError *processingError = nil;
    if (@available(macOS 10.15, *)) {
        if (![input setVoiceProcessingEnabled:YES error:&processingError]) {
            [self emit:4 audio:NULL length:0 error:processingError.localizedDescription ?: @"Could not enable microphone echo cancellation."];
            return;
        }
    }

    AVAudioFormat *inputFormat = [input outputFormatForBus:0];
    if (inputFormat.sampleRate <= 0 || inputFormat.channelCount == 0) {
        [self emit:4 audio:NULL length:0 error:@"No microphone input format is available."];
        return;
    }
    AVAudioFormat *voiceFormat = [[AVAudioFormat alloc]
        initWithCommonFormat:AVAudioPCMFormatInt16
                  sampleRate:24000
                    channels:1
                 interleaved:NO];
    if (voiceFormat == nil) {
        [self emit:4 audio:NULL length:0 error:@"Could not create the 24 kHz voice audio format."];
        return;
    }
    AVAudioConverter *converter = [[AVAudioConverter alloc]
        initFromFormat:inputFormat
              toFormat:voiceFormat];
    if (converter == nil) {
        [self emit:4 audio:NULL length:0 error:@"Could not prepare microphone resampling."];
        return;
    }

    AVAudioPlayerNode *player = [[AVAudioPlayerNode alloc] init];
    [engine attachNode:player];
    NSError *connectionError = nil;
    BOOL connected = NO;
    if (@available(macOS 27.0, *)) {
        connected = [engine connect:player
                                 to:engine.mainMixerNode
                             format:voiceFormat
                              error:&connectionError];
    } else {
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
        [engine connect:player to:engine.mainMixerNode format:voiceFormat];
#pragma clang diagnostic pop
        connected = YES;
    }
    if (!connected) {
        [self emit:4 audio:NULL length:0 error:connectionError.localizedDescription ?: @"Could not connect speaker audio."];
        return;
    }
    __weak AHEADVoiceCapture *weakSelf = self;
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
    [input installTapOnBus:0 bufferSize:1024 format:inputFormat
                     block:^(AVAudioPCMBuffer *buffer, AVAudioTime *when) {
        (void)when;
        AHEADVoiceCapture *strongSelf = weakSelf;
        if (strongSelf != nil) {
            @autoreleasepool {
                [strongSelf consume:buffer];
            }
        }
    }];
#pragma clang diagnostic pop

    self.engine = engine;
    self.converter = converter;
    self.voiceFormat = voiceFormat;
    self.player = player;
    [engine prepare];
    NSError *startError = nil;
    if (![engine startAndReturnError:&startError]) {
        [input removeTapOnBus:0];
        self.engine = nil;
        self.converter = nil;
        self.voiceFormat = nil;
        self.player = nil;
        [self emit:4 audio:NULL length:0 error:startError.localizedDescription ?: @"Could not start microphone and speaker audio."];
        return;
    }
    [self observeAudioLifecycleForEngine:engine];
    [self emit:1 audio:NULL length:0 error:nil];
}

- (void)observeAudioLifecycleForEngine:(AVAudioEngine *)engine {
    __weak AHEADVoiceCapture *weakSelf = self;
    self.audioConfigurationObserver = [[NSNotificationCenter defaultCenter]
        addObserverForName:AVAudioEngineConfigurationChangeNotification
                    object:engine
                     queue:nil
                usingBlock:^(NSNotification *notification) {
        (void)notification;
        AHEADVoiceCapture *strongSelf = weakSelf;
        if (strongSelf != nil) {
            dispatch_async(strongSelf.queue, ^{
                [strongSelf emit:4 audio:NULL length:0 error:@"The audio device changed. End this call and reconnect to resume voice." ];
            });
        }
    }];
    self.systemSleepObserver = [[[NSWorkspace sharedWorkspace] notificationCenter]
        addObserverForName:NSWorkspaceWillSleepNotification
                    object:nil
                     queue:nil
                usingBlock:^(NSNotification *notification) {
        (void)notification;
        AHEADVoiceCapture *strongSelf = weakSelf;
        if (strongSelf != nil) {
            dispatch_async(strongSelf.queue, ^{
                [strongSelf emit:4 audio:NULL length:0 error:@"System sleep ended audio capture. Start a new voice call after waking." ];
            });
        }
    }];
}

- (void)consume:(AVAudioPCMBuffer *)buffer {
    if (self.stopped || self.muted || self.converter == nil || self.voiceFormat == nil) {
        return;
    }
    AVAudioFormat *sourceFormat = buffer.format;
    AVAudioFrameCount outputCapacity = (AVAudioFrameCount)
        ceil((double)buffer.frameLength * 24000.0 / sourceFormat.sampleRate) + 16;
    AVAudioPCMBuffer *converted = [[AVAudioPCMBuffer alloc]
        initWithPCMFormat:self.voiceFormat
            frameCapacity:outputCapacity];
    if (converted == nil) {
        [self emit:4 audio:NULL length:0 error:@"Could not allocate a microphone audio frame."];
        return;
    }
    __block BOOL suppliedInput = NO;
    NSError *conversionError = nil;
    AVAudioConverterOutputStatus status = [self.converter
        convertToBuffer:converted
                  error:&conversionError
     withInputFromBlock:^AVAudioBuffer *(AVAudioPacketCount inNumberOfPackets,
                                         AVAudioConverterInputStatus *outStatus) {
        (void)inNumberOfPackets;
        if (suppliedInput) {
            *outStatus = AVAudioConverterInputStatus_NoDataNow;
            return nil;
        }
        suppliedInput = YES;
        *outStatus = AVAudioConverterInputStatus_HaveData;
        return buffer;
    }];
    if (status == AVAudioConverterOutputStatus_Error) {
        [self emit:4 audio:NULL length:0 error:conversionError.localizedDescription ?: @"Could not resample microphone audio."];
        return;
    }
    if (converted.frameLength == 0 || converted.int16ChannelData == NULL) {
        return;
    }

    int16_t *samples = converted.int16ChannelData[0];
    double squareTotal = 0;
    for (AVAudioFrameCount index = 0; index < converted.frameLength; index++) {
        const double sample = (double)samples[index] / 32768.0;
        squareTotal += sample * sample;
    }
    const double rootMeanSquare = sqrt(squareTotal / converted.frameLength);
    const NSTimeInterval duration = (double)converted.frameLength / 24000.0;
    if (rootMeanSquare >= 0.012) {
        self.silenceSeconds = 0;
        if (!self.speechActive) {
            self.speechActive = YES;
            [self emit:2 audio:NULL length:0 error:nil];
        }
    } else if (self.speechActive) {
        self.silenceSeconds += duration;
        if (self.silenceSeconds >= 0.8) {
            self.speechActive = NO;
            self.silenceSeconds = 0;
            [self emit:5 audio:NULL length:0 error:nil];
        }
    }
    [self emit:3
         audio:(const uint8_t *)samples
        length:(size_t)converted.frameLength * sizeof(int16_t)
         error:nil];
}

- (void)setCaptureMuted:(BOOL)muted {
    dispatch_async(self.queue, ^{
        if (self.stopped || self.muted == muted) {
            return;
        }
        self.muted = muted;
        const BOOL wasSpeaking = self.speechActive;
        self.speechActive = NO;
        self.silenceSeconds = 0;
        if (wasSpeaking) {
            [self emit:5 audio:NULL length:0 error:nil];
        }
    });
}

- (void)playAudio:(NSData *)audio {
    dispatch_async(self.queue, ^{
        if (self.stopped || self.voiceFormat == nil || self.player == nil || audio.length == 0) {
            return;
        }
        if (self.scheduledAudioBytes + audio.length > 96 * 1024) {
            [self emit:4 audio:NULL length:0 error:@"Speaker playback fell behind the live audio stream."];
            return;
        }
        const AVAudioFrameCount frames = (AVAudioFrameCount)(audio.length / sizeof(int16_t));
        AVAudioPCMBuffer *buffer = [[AVAudioPCMBuffer alloc]
            initWithPCMFormat:self.voiceFormat
                frameCapacity:frames];
        if (buffer == nil || buffer.int16ChannelData == NULL) {
            [self emit:4 audio:NULL length:0 error:@"Could not queue speaker audio."];
            return;
        }
        memcpy(buffer.int16ChannelData[0], audio.bytes, audio.length);
        buffer.frameLength = frames;
        self.scheduledBuffers += 1;
        self.scheduledAudioBytes += audio.length;
        const NSUInteger audioLength = audio.length;
        const NSUInteger audioFrames = frames;
        const uint64_t playbackGeneration = self.playbackGeneration;
        [self.player scheduleBuffer:buffer
            completionCallbackType:AVAudioPlayerNodeCompletionDataPlayedBack
                    completionHandler:^(AVAudioPlayerNodeCompletionCallbackType callbackType) {
            (void)callbackType;
            dispatch_async(self.queue, ^{
                if (self.playbackGeneration != playbackGeneration) {
                    return;
                }
                [self emit:8 audio:NULL length:audioFrames value:playbackGeneration error:nil];
                if (self.scheduledBuffers > 0) {
                    self.scheduledBuffers -= 1;
                }
                self.scheduledAudioBytes = self.scheduledAudioBytes > audioLength
                    ? self.scheduledAudioBytes - audioLength
                    : 0;
                if (self.scheduledBuffers == 0 && self.outputActive) {
                    self.outputActive = NO;
                    [self emit:7 audio:NULL length:0 error:nil];
                }
            });
        }];
        if (!self.player.isPlaying) {
            NSError *playbackError = nil;
            BOOL started = NO;
            if (@available(macOS 27.0, *)) {
                started = [self.player playAndReturnError:&playbackError];
            } else {
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
                [self.player play];
#pragma clang diagnostic pop
                started = YES;
            }
            if (!started) {
                [self.player stop];
                [self.player reset];
                self.scheduledBuffers = 0;
                self.scheduledAudioBytes = 0;
                [self emit:4 audio:NULL length:0 error:playbackError.localizedDescription ?: @"Could not start speaker playback."];
                return;
            }
        }
        if (!self.outputActive) {
            self.outputActive = YES;
            [self emit:6 audio:NULL length:0 error:nil];
        }
    });
}

- (uint64_t)stopPlayback {
    __block uint64_t generation = 0;
    dispatch_sync(self.queue, ^{
        self.playbackGeneration += 1;
        generation = self.playbackGeneration;
        [self.player stop];
        [self.player reset];
        self.scheduledBuffers = 0;
        self.scheduledAudioBytes = 0;
        if (self.outputActive) {
            self.outputActive = NO;
            [self emit:7 audio:NULL length:0 error:nil];
        }
    });
    return generation;
}

- (void)stop {
    dispatch_sync(self.queue, ^{
        if (self.stopped) {
            return;
        }
        self.stopped = YES;
        AVAudioEngine *engine = self.engine;
        AVAudioPlayerNode *player = self.player;
        if (self.audioConfigurationObserver != nil) {
            [[NSNotificationCenter defaultCenter]
                removeObserver:self.audioConfigurationObserver];
            self.audioConfigurationObserver = nil;
        }
        if (self.systemSleepObserver != nil) {
            [[[NSWorkspace sharedWorkspace] notificationCenter]
                removeObserver:self.systemSleepObserver];
            self.systemSleepObserver = nil;
        }
        self.engine = nil;
        self.converter = nil;
        self.voiceFormat = nil;
        self.player = nil;
        if (player != nil) {
            [player stop];
        }
        if (engine != nil) {
            [engine.inputNode removeTapOnBus:0];
            [engine stop];
        }
    });
}

- (void)dealloc {
    if (self.releaseCallback != NULL) {
        self.releaseCallback(self.context);
    }
}

@end

void *ahead_voice_create(
    AHEADVoiceEventCallback callback,
    AHEADVoiceReleaseCallback release_callback,
    void *context
) {
    AHEADVoiceCapture *capture = [[AHEADVoiceCapture alloc] init];
    capture.callback = callback;
    capture.releaseCallback = release_callback;
    capture.context = context;
    capture.queue = dispatch_queue_create("io.ahead.voice", DISPATCH_QUEUE_SERIAL);
    return (__bridge_retained void *)capture;
}

void ahead_voice_start(void *handle) {
    [(__bridge AHEADVoiceCapture *)handle start];
}

void ahead_voice_set_muted(void *handle, bool muted) {
    [(__bridge AHEADVoiceCapture *)handle setCaptureMuted:muted];
}

void ahead_voice_play(void *handle, const uint8_t *audio, size_t length) {
    if (audio == NULL || length == 0 || length % sizeof(int16_t) != 0) {
        return;
    }
    NSData *copy = [NSData dataWithBytes:audio length:length];
    [(__bridge AHEADVoiceCapture *)handle playAudio:copy];
}

uint64_t ahead_voice_stop_playback(void *handle) {
    return [(__bridge AHEADVoiceCapture *)handle stopPlayback];
}

void ahead_voice_stop(void *handle) {
    [(__bridge AHEADVoiceCapture *)handle stop];
}

void ahead_voice_destroy(void *handle) {
    (void)(__bridge_transfer AHEADVoiceCapture *)handle;
}
