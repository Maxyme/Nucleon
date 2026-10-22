#import "fsr4_depth_compat.h"
#include <assert.h>

@interface SetterOnlyScaler : NSObject
@property(nonatomic) BOOL recorded;
- (void)setDepthReversed:(BOOL)value;
@end
@implementation SetterOnlyScaler
- (void)setDepthReversed:(BOOL)value { self.recorded = value; }
@end

@interface InheritedSetterScaler : SetterOnlyScaler
@end
@implementation InheritedSetterScaler
@end

@interface LateInheritedSetterScaler : SetterOnlyScaler
@end
@implementation LateInheritedSetterScaler
@end

@interface GetterScaler : NSObject
@property(nonatomic, getter=isDepthReversed) BOOL depthReversed;
@end
@implementation GetterScaler
@end

int main(void) {
    @autoreleasepool {
        SetterOnlyScaler *scaler = [SetterOnlyScaler new];
        assert(nucleon_depth_reversed(scaler) == -1);
        nucleon_capture_depth_setter([InheritedSetterScaler class]);
        InheritedSetterScaler *child = [InheritedSetterScaler new];
        [child setDepthReversed:YES];
        assert(child.recorded && nucleon_depth_reversed(child) == 1);
        [scaler setDepthReversed:YES];
        assert(scaler.recorded && nucleon_depth_reversed(scaler) == -1);
        nucleon_capture_depth_setter([SetterOnlyScaler class]);
        nucleon_capture_depth_setter([SetterOnlyScaler class]);
        [scaler setDepthReversed:YES];
        assert(scaler.recorded && nucleon_depth_reversed(scaler) == 1);
        [scaler setDepthReversed:NO];
        assert(!scaler.recorded && nucleon_depth_reversed(scaler) == 0);
        nucleon_capture_depth_setter([LateInheritedSetterScaler class]);
        LateInheritedSetterScaler *late = [LateInheritedSetterScaler new];
        [late setDepthReversed:YES];
        assert(late.recorded && nucleon_depth_reversed(late) == 1);
        [late setDepthReversed:NO];
        assert(!late.recorded && nucleon_depth_reversed(late) == 0);
        GetterScaler *getter = [GetterScaler new];
        getter.depthReversed = YES;
        assert(nucleon_depth_reversed(getter) == 1);
        getter.depthReversed = NO;
        assert(nucleon_depth_reversed(getter) == 0);
        puts("Depth compatibility checks passed");
    }
    return 0;
}
