// macOS 27.2's _MFXTemporalScalingEffectBBR exposes setDepthReversed: but
// does not answer the public protocol's isDepthReversed getter. Remember the
// actual setter value rather than guessing the game's depth convention.
#import <Foundation/Foundation.h>
#import <objc/message.h>
#import <objc/runtime.h>

static const void *nucleon_depth_key = &nucleon_depth_key;
typedef struct { Class cls; IMP setter; } NucleonDepthSlot;
static NucleonDepthSlot nucleon_depth_slots[4];

static int nucleon_depth_reversed(id scaler) {
    for (const char **name = (const char *[]){"isDepthReversed", "depthReversed", NULL}; *name; name++) {
        SEL selector = sel_registerName(*name);
        if ([scaler respondsToSelector:selector])
            return ((BOOL (*)(id, SEL))objc_msgSend)(scaler, selector) ? 1 : 0;
    }
    NSNumber *captured = objc_getAssociatedObject(scaler, nucleon_depth_key);
    return captured ? captured.boolValue : -1;
}

static void nucleon_set_depth(id scaler, SEL selector, BOOL reversed) {
    for (Class cls = object_getClass(scaler); cls; cls = class_getSuperclass(cls)) {
        for (int i = 0; i < 4; i++) {
            if (nucleon_depth_slots[i].cls == cls) {
                ((void (*)(id, SEL, BOOL))nucleon_depth_slots[i].setter)(scaler, selector, reversed);
                objc_setAssociatedObject(scaler, nucleon_depth_key, @(reversed), OBJC_ASSOCIATION_RETAIN_NONATOMIC);
                return;
            }
        }
    }
}

static void nucleon_capture_depth_setter(Class cls) {
    for (int i = 0; i < 4; i++)
        if (nucleon_depth_slots[i].cls == cls) return;
    SEL selector = sel_registerName("setDepthReversed:");
    Method method = class_getInstanceMethod(cls, selector);
    if (!method) return;
    IMP original = method_getImplementation(method);
    // A superclass may already be hooked. Retain its original setter, rather
    // than recording our own interceptor and recursively calling ourselves.
    if (original == (IMP)nucleon_set_depth) {
        for (Class parent = class_getSuperclass(cls); parent; parent = class_getSuperclass(parent)) {
            for (int i = 0; i < 4; i++)
                if (nucleon_depth_slots[i].cls == parent) original = nucleon_depth_slots[i].setter;
            if (original != (IMP)nucleon_set_depth) break;
        }
        if (original == (IMP)nucleon_set_depth) return;
    }
    for (int i = 0; i < 4; i++) {
        if (nucleon_depth_slots[i].cls) continue;
        nucleon_depth_slots[i] = (NucleonDepthSlot){cls, original};
        // Give an inherited setter its own subclass slot; do not replace a
        // superclass implementation used by unrelated MetalFX scaler classes.
        if (!class_addMethod(cls, selector, (IMP)nucleon_set_depth, method_getTypeEncoding(method)))
            method_setImplementation(class_getInstanceMethod(cls, selector), (IMP)nucleon_set_depth);
        return;
    }
}
