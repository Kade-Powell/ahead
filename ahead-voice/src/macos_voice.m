#import <AVFoundation/AVFoundation.h>
#import <Foundation/Foundation.h>
#import <Speech/Speech.h>
#include <math.h>
#include <stdint.h>

typedef void (*AHEADVoiceEventCallback)(
    void *context,
    int32_t event,
    const char *text,
    const char *error
);
typedef void (*AHEADVoiceReleaseCallback)(const void *context);

@interface AHEADVoiceCapture : NSObject
@property(nonatomic, assign) AHEADVoiceEventCallback callback;
@property(nonatomic, assign) AHEADVoiceReleaseCallback releaseCallback;
@property(nonatomic, assign) void *context;
@property(nonatomic, strong) dispatch_queue_t queue;
@property(nonatomic, strong) AVAudioEngine *engine;
@property(nonatomic, strong) SFSpeechRecognizer *recognizer;
@property(atomic, strong) SFSpeechAudioBufferRecognitionRequest *request;
@property(nonatomic, strong) SFSpeechRecognitionTask *task;
@property(nonatomic, copy) NSString *lastTranscript;
@property(atomic, assign) BOOL stopped;
@property(atomic, assign) BOOL muted;
@property(atomic, assign) BOOL speechActive;
@property(atomic, assign) BOOL segmentEnding;
@property(nonatomic, assign) uint64_t recognitionGeneration;
@property(nonatomic, assign) NSTimeInterval silenceSeconds;
@end

@implementation AHEADVoiceCapture

- (void)emit:(int32_t)event text:(NSString *)text error:(NSString *)error {
    if (self.callback == NULL || self.stopped) {
        return;
    }
    self.callback(
        self.context,
        event,
        text == nil ? "" : text.UTF8String,
        error == nil ? "" : error.UTF8String
    );
}

- (void)start {
    dispatch_async(self.queue, ^{
        if (@available(macOS 10.15, *)) {
            [AVCaptureDevice requestAccessForMediaType:AVMediaTypeAudio
                                     completionHandler:^(BOOL granted) {
                dispatch_async(self.queue, ^{
                    if (self.stopped) {
                        return;
                    }
                    if (!granted) {
                        [self emit:4 text:nil error:@"Microphone access was denied. Enable it in System Settings > Privacy & Security > Microphone."];
                        return;
                    }
                    [SFSpeechRecognizer requestAuthorization:^(SFSpeechRecognizerAuthorizationStatus status) {
                        dispatch_async(self.queue, ^{
                            if (self.stopped) {
                                return;
                            }
                            if (status != SFSpeechRecognizerAuthorizationStatusAuthorized) {
                                [self emit:4 text:nil error:@"Speech recognition access was denied or restricted."];
                                return;
                            }
                            [self startRecognizingOnDevice];
                        });
                    }];
                });
            }];
        } else {
            [self emit:4 text:nil error:@"Local speech input requires macOS 10.15 or newer."];
        }
    });
}

- (void)startRecognizingOnDevice API_AVAILABLE(macos(10.15)) {
    NSLocale *locale = [NSLocale currentLocale];
    self.recognizer = [[SFSpeechRecognizer alloc] initWithLocale:locale];
    if (self.recognizer == nil || !self.recognizer.isAvailable) {
        [self emit:4 text:nil error:@"The system speech recognizer is unavailable for the current language."];
        return;
    }
    if (!self.recognizer.supportsOnDeviceRecognition) {
        [self emit:4 text:nil error:@"The current system language has no on-device recognizer. AHEAD will not send microphone audio over the network."];
        return;
    }

    self.engine = [[AVAudioEngine alloc] init];
    AVAudioInputNode *input = self.engine.inputNode;
    AVAudioFormat *format = [input outputFormatForBus:0];
    if (format.sampleRate <= 0 || format.channelCount == 0) {
        [self emit:4 text:nil error:@"No microphone input format is available."];
        self.engine = nil;
        return;
    }
    __weak AHEADVoiceCapture *weakSelf = self;
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
    [input installTapOnBus:0 bufferSize:1024 format:format
                     block:^(AVAudioPCMBuffer *buffer, AVAudioTime *when) {
        (void)when;
        AHEADVoiceCapture *strongSelf = weakSelf;
        if (strongSelf == nil) {
            return;
        }
        @autoreleasepool {
            [strongSelf consume:buffer sampleRate:format.sampleRate];
        }
    }];
#pragma clang diagnostic pop
    [self.engine prepare];
    NSError *startError = nil;
    if (![self.engine startAndReturnError:&startError]) {
        [input removeTapOnBus:0];
        self.engine = nil;
        [self emit:4 text:nil error:startError.localizedDescription ?: @"Could not start the microphone."];
        return;
    }
    [self startRecognitionSegment];
}

- (void)startRecognitionSegment {
    if (self.stopped || self.muted || self.recognizer == nil) {
        return;
    }
    SFSpeechAudioBufferRecognitionRequest *request =
        [[SFSpeechAudioBufferRecognitionRequest alloc] init];
    request.shouldReportPartialResults = YES;
    request.requiresOnDeviceRecognition = YES;
    uint64_t generation = ++self.recognitionGeneration;
    @synchronized (self) {
        if (self.stopped || self.muted) {
            return;
        }
        self.request = request;
        self.speechActive = NO;
        self.segmentEnding = NO;
        self.silenceSeconds = 0;
        self.lastTranscript = @"";
    }
    __weak AHEADVoiceCapture *weakSelf = self;
    self.task = [self.recognizer recognitionTaskWithRequest:request
                                              resultHandler:^(SFSpeechRecognitionResult *result, NSError *error) {
        AHEADVoiceCapture *strongSelf = weakSelf;
        if (strongSelf == nil || strongSelf.stopped) {
            return;
        }
        dispatch_async(strongSelf.queue, ^{
            if (strongSelf.stopped || generation != strongSelf.recognitionGeneration) {
                return;
            }
            if (result != nil) {
                NSString *text = result.bestTranscription.formattedString ?: @"";
                strongSelf.lastTranscript = text;
                [strongSelf emit:1 text:text error:nil];
                if (result.isFinal) {
                    if (text.length > 0) {
                        [strongSelf emit:3 text:text error:nil];
                    }
                    [strongSelf finishRecognitionSegment];
                    return;
                }
            }
            if (error != nil) {
                if (strongSelf.segmentEnding || strongSelf.muted) {
                    if (strongSelf.lastTranscript.length > 0) {
                        [strongSelf emit:3 text:strongSelf.lastTranscript error:nil];
                        strongSelf.lastTranscript = @"";
                    }
                    [strongSelf finishRecognitionSegment];
                } else {
                    [strongSelf emit:4 text:nil error:error.localizedDescription ?: @"On-device transcription failed."];
                }
            }
        });
    }];
}

- (void)finishRecognitionSegment {
    BOOL shouldRestart = NO;
    @synchronized (self) {
        self.task = nil;
        self.request = nil;
        self.segmentEnding = NO;
        shouldRestart = !self.stopped && !self.muted;
    }
    if (shouldRestart) {
        [self startRecognitionSegment];
    }
}

- (void)consume:(AVAudioPCMBuffer *)buffer sampleRate:(double)sampleRate {
    @synchronized (self) {
        if (self.stopped || self.muted || self.segmentEnding || self.request == nil) {
            return;
        }
        [self.request appendAudioPCMBuffer:buffer];
        const float *samples = buffer.floatChannelData == NULL ? NULL : buffer.floatChannelData[0];
        if (samples == NULL || buffer.frameLength == 0 || sampleRate <= 0) {
            return;
        }
        double squareTotal = 0;
        for (AVAudioFrameCount index = 0; index < buffer.frameLength; index++) {
            double sample = samples[index];
            squareTotal += sample * sample;
        }
        double rootMeanSquare = sqrt(squareTotal / buffer.frameLength);
        NSTimeInterval duration = (double)buffer.frameLength / sampleRate;
        if (rootMeanSquare >= 0.012) {
            self.silenceSeconds = 0;
            if (!self.speechActive) {
                self.speechActive = YES;
                [self emit:2 text:nil error:nil];
            }
        } else if (self.speechActive) {
            self.silenceSeconds += duration;
            if (self.silenceSeconds >= 0.8) {
                self.segmentEnding = YES;
                [self.request endAudio];
            }
        }
    }
}

- (void)setCaptureMuted:(BOOL)muted {
    dispatch_async(self.queue, ^{
        SFSpeechAudioBufferRecognitionRequest *request = nil;
        BOOL shouldStart = NO;
        @synchronized (self) {
            if (self.stopped || self.muted == muted) {
                return;
            }
            self.muted = muted;
            if (muted) {
                self.segmentEnding = YES;
                request = self.request;
            } else {
                shouldStart = self.task == nil || self.request == nil;
            }
        }
        if (muted) {
            [request endAudio];
        } else if (shouldStart) {
            [self startRecognitionSegment];
        }
    });
}

- (void)stop {
    dispatch_sync(self.queue, ^{
        AVAudioEngine *engine = nil;
        SFSpeechAudioBufferRecognitionRequest *request = nil;
        SFSpeechRecognitionTask *task = nil;
        NSString *transcript = nil;
        @synchronized (self) {
            if (self.stopped) {
                return;
            }
            self.stopped = YES;
            engine = self.engine;
            request = self.request;
            task = self.task;
            transcript = self.lastTranscript;
            self.request = nil;
            self.task = nil;
            self.engine = nil;
        }
        if (transcript.length > 0) {
            self.callback(self.context, 3, transcript.UTF8String, "");
        }
        [engine.inputNode removeTapOnBus:0];
        [engine stop];
        [request endAudio];
        [task cancel];
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

void ahead_voice_stop(void *handle) {
    [(__bridge AHEADVoiceCapture *)handle stop];
}

void ahead_voice_destroy(void *handle) {
    (void)(__bridge_transfer AHEADVoiceCapture *)handle;
}
