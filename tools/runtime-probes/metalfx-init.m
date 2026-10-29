/* Bounded public-API MetalFX initialization reproduction. No Wine or game required.
 * Compare arm64 with x86_64/Rosetta; no rendering or image-quality validation.
 * MetalFX may populate its own normal system caches. This probe never clears them.
 */
#import <Foundation/Foundation.h>
#import <Metal/Metal.h>
#import <MetalFX/MetalFX.h>
#import <unistd.h>
#include <time.h>
static double now(void) { struct timespec t; clock_gettime(CLOCK_MONOTONIC, &t); return t.tv_sec+t.tv_nsec/1e9; }
int main(int argc, const char **argv) { @autoreleasepool {
    if (argc > 3 || (argc > 1 && strcmp(argv[1], "metal3") && strcmp(argv[1], "metal4")) ||
        (argc > 2 && strcmp(argv[2], "sync") && strcmp(argv[2], "async"))) {
        fprintf(stderr, "usage: metalfx-init [metal3|metal4] [sync|async]\n"); return 1;
    }
    BOOL metal4 = argc > 1 && strcmp(argv[1], "metal4") == 0;
    BOOL sync = argc < 3 || strcmp(argv[2], "async") != 0;
    id<MTLDevice> device = MTLCreateSystemDefaultDevice();
    if (!device || ![MTLFXTemporalScalerDescriptor supportsDevice:device] ||
        (metal4 && ![MTLFXTemporalScalerDescriptor supportsMetal4FX:device])) return 2;
    printf("pid=%d device=%s arch=%s api=%s synchronous=%d\n", getpid(), device.name.UTF8String,
#ifdef __arm64__
        "arm64",
#else
        "x86_64",
#endif
        metal4?"Metal4":"Metal3", sync); fflush(stdout);
    MTLFXTemporalScalerDescriptor *desc = [MTLFXTemporalScalerDescriptor new];
    desc.inputWidth=1708; desc.inputHeight=960; desc.outputWidth=2560; desc.outputHeight=1440;
    desc.colorTextureFormat=MTLPixelFormatRGBA16Float; desc.outputTextureFormat=MTLPixelFormatRGBA16Float;
    desc.depthTextureFormat=MTLPixelFormatR32Float; desc.motionTextureFormat=MTLPixelFormatRG16Float;
    desc.autoExposureEnabled=YES; desc.requiresSynchronousInitialization=sync;
    id scaler=nil; double start=now();
    if (metal4) {
        NSError *error=nil;
        id<MTL4Compiler> compiler=[device newCompilerWithDescriptor:[MTL4CompilerDescriptor new] error:&error];
        if (!compiler) { fprintf(stderr,"compiler error: %s\n", error.description.UTF8String); return 3; }
        scaler=[desc newTemporalScalerWithDevice:device compiler:compiler];
    } else scaler=[desc newTemporalScalerWithDevice:device];
    printf("creation_ms=%.3f created=%d\n", (now()-start)*1000, scaler!=nil); fflush(stdout);
    // Retain the scaler while asynchronous framework work has time to complete.
    [NSThread sleepForTimeInterval:8];
    printf("completed_ms=%.3f\n", (now()-start)*1000);
    return scaler?0:4;
} }
