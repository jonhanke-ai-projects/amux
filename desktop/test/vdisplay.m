// vdisplay <name> <serial> <w> <h>: a virtual monitor that exists while this runs.
// The same serial gives the same display UUID, so stopping and starting it
// is a monitor being unplugged and plugged back in. Used by workspace-e2e.py.
// macOS removes it only while the main display is awake; under a sleeping
// monitor it stays connected after this exits, until the monitor wakes.
#import <Foundation/Foundation.h>
#import <CoreGraphics/CoreGraphics.h>
@interface CGVirtualDisplayDescriptor : NSObject
@property(retain) dispatch_queue_t queue; @property(retain) NSString *name;
@property unsigned int maxPixelsWide, maxPixelsHigh, vendorID, productID, serialNum; @property CGSize sizeInMillimeters;
@end
@interface CGVirtualDisplayMode : NSObject
- (instancetype)initWithWidth:(unsigned)w height:(unsigned)h refreshRate:(double)r;
@end
@interface CGVirtualDisplaySettings : NSObject
@property(retain) NSArray *modes; @property unsigned int hiDPI;
@end
@interface CGVirtualDisplay : NSObject
- (instancetype)initWithDescriptor:(CGVirtualDisplayDescriptor *)d;
- (BOOL)applySettings:(CGVirtualDisplaySettings *)s;
@property(readonly) unsigned int displayID;
@end
int main(int argc, char **argv) { @autoreleasepool {
  CGVirtualDisplayDescriptor *d = [CGVirtualDisplayDescriptor new];
  d.queue = dispatch_get_main_queue(); d.name = [NSString stringWithUTF8String:argv[1]];
  unsigned w = atoi(argv[3]), h = atoi(argv[4]);
  d.maxPixelsWide = w; d.maxPixelsHigh = h; d.sizeInMillimeters = CGSizeMake(600, 340);
  d.vendorID = 0x1234; d.productID = 0x5678; d.serialNum = atoi(argv[2]);
  CGVirtualDisplay *disp = [[CGVirtualDisplay alloc] initWithDescriptor:d];
  CGVirtualDisplaySettings *s = [CGVirtualDisplaySettings new];
  s.hiDPI = 0; s.modes = @[[[CGVirtualDisplayMode alloc] initWithWidth:w height:h refreshRate:60]];
  BOOL ok = [disp applySettings:s];
  printf("display %u applied=%d\n", disp.displayID, ok); fflush(stdout);
  dispatch_main();
}}
