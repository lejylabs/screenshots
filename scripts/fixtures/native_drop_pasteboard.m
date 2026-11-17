// Private named-pasteboard provider. No general clipboard, window, input or network access.
#import <AppKit/AppKit.h>
#import <Foundation/Foundation.h>
#import <sys/mman.h>
#import <unistd.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

@interface OwnedDropProvider : NSObject <NSPasteboardItemDataProvider>
@property(nonatomic, strong) NSString *mode;
@property(nonatomic, strong) NSPasteboard *board;
@end
@implementation OwnedDropProvider
- (void)pasteboard:(NSPasteboard *)pasteboard item:(NSPasteboardItem *)item provideDataForType:(NSPasteboardType)type {
    (void)pasteboard;
    printf("requested\n"); fflush(stdout);
    if ([self.mode isEqualToString:@"stall"]) {
        [NSThread sleepForTimeInterval:30.0];
        return;
    }
    if ([self.mode isEqualToString:@"change"]) [self.board clearContents];
    if ([self.mode isEqualToString:@"over-limit"]) {
        size_t count = 32u * 1024u * 1024u + 1u;
        void *bytes = mmap(NULL, count, PROT_READ, MAP_PRIVATE | MAP_ANON, -1, 0);
        if (bytes != MAP_FAILED) {
            [item setData:[NSData dataWithBytesNoCopy:bytes length:count freeWhenDone:NO] forType:type];
        }
        return;
    }
    // Two synthetic opaque RGBA pixels; generated fixture, not user content.
    NSData *png = [[NSData alloc] initWithBase64EncodedString:@"iVBORw0KGgoAAAANSUhEUgAAAAIAAAABCAYAAAD0In+KAAAAAXNSR0IArs4c6QAAABFJREFUeJxj4BZX/a9t7d4AAAlGAnTWROZ6AAAAAElFTkSuQmCC" options:0];
    [item setData:png forType:type];
}
@end

int main(int argc, const char **argv) {
    @autoreleasepool {
        if (argc != 2 || strcmp(getenv("SCREENSHOTS_NATIVE_DROP_PRIVATE") ?: "", "1") != 0) return 2;
        NSString *mode = [NSString stringWithUTF8String:argv[1]];
        if (![@[@"direct", @"uri", @"file", @"stall", @"over-limit", @"change"] containsObject:mode]) return 2;
        NSPasteboard *board = [NSPasteboard pasteboardWithUniqueName];
        if (!board || [board.name isEqualToString:NSPasteboardNameGeneral]) return 3;
        [board clearContents];
        NSPasteboardItem *item = [NSPasteboardItem new];
        OwnedDropProvider *provider = [OwnedDropProvider new]; provider.mode = mode; provider.board = board;
        if ([mode isEqualToString:@"uri"]) {
            [item setData:[@"https://example.invalid/image.png?private-query-token" dataUsingEncoding:NSUTF8StringEncoding] forType:@"public.url"];
        } else if ([mode isEqualToString:@"file"]) {
            [item setData:[@"file:///private/fixture-%FF.png" dataUsingEncoding:NSUTF8StringEncoding] forType:@"public.file-url"];
        } else {
            [item setDataProvider:provider forTypes:@[@"public.png"]];
        }
        if (![board writeObjects:@[item]]) return 4;
        NSDictionary *descriptor = @{@"name":board.name, @"change_count":@(board.changeCount)};
        NSData *json = [NSJSONSerialization dataWithJSONObject:descriptor options:0 error:nil];
        fwrite(json.bytes, 1, json.length, stdout); printf("\n"); fflush(stdout);
        NSDate *deadline = [NSDate dateWithTimeIntervalSinceNow:25.0];
        while (deadline.timeIntervalSinceNow > 0) {
            @autoreleasepool {
                [[NSRunLoop currentRunLoop] runMode:NSDefaultRunLoopMode beforeDate:deadline];
            }
        }
        // Releasing this private name cannot clear another application's pasteboard.
        [board releaseGlobally];
    }
    return 0;
}
