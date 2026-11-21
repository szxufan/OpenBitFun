// Disposable native file chooser: exercise the real remote AppKit panel tree.
#import <AppKit/AppKit.h>

int main(int argc, const char **argv) {
    @autoreleasepool {
        if (argc != 3) return 2;
        NSApplication *app = NSApplication.sharedApplication;
        app.activationPolicy = NSApplicationActivationPolicyRegular;
        NSWindow *window = [[NSWindow alloc] initWithContentRect:NSMakeRect(100, 200, 1000, 700)
            styleMask:NSWindowStyleMaskTitled backing:NSBackingStoreBuffered defer:NO];
        window.title = @"OpenBitFun File Panel Fixture";
        window.releasedWhenClosed = NO;
        window.collectionBehavior = NSWindowCollectionBehaviorCanJoinAllSpaces | NSWindowCollectionBehaviorFullScreenAuxiliary;
        [window orderFrontRegardless];
        NSWindow *sibling = [[NSWindow alloc] initWithContentRect:NSMakeRect(1100, 200, 250, 200)
            styleMask:NSWindowStyleMaskTitled backing:NSBackingStoreBuffered defer:NO];
        sibling.title = @"Unbound File Panel Fixture";
        sibling.releasedWhenClosed = NO;
        NSButton *unbound = [NSButton buttonWithTitle:@"Unbound semantic action" target:nil action:nil];
        unbound.frame = NSMakeRect(10, 50, 230, 32);
        [sibling.contentView addSubview:unbound];
        sibling.collectionBehavior = NSWindowCollectionBehaviorCanJoinAllSpaces | NSWindowCollectionBehaviorFullScreenAuxiliary;
        [sibling orderFrontRegardless];
        NSOpenPanel *panel = NSOpenPanel.openPanel;
        panel.collectionBehavior = NSWindowCollectionBehaviorCanJoinAllSpaces | NSWindowCollectionBehaviorFullScreenAuxiliary;
        panel.title = @"OpenBitFun Directory Fixture";
        panel.canChooseFiles = NO;
        panel.canChooseDirectories = YES;
        panel.directoryURL = [NSURL fileURLWithPath:[NSString stringWithUTF8String:argv[2]]];
        [panel beginSheetModalForWindow:window completionHandler:^(NSModalResponse response) {}];
        dispatch_after(dispatch_time(DISPATCH_TIME_NOW, NSEC_PER_SEC), dispatch_get_main_queue(), ^{
            NSDictionary *state = @{@"pid": @(getpid()), @"window_id": @(window.windowNumber),
                @"panel_window_id": @(panel.windowNumber), @"sibling_window_id": @(sibling.windowNumber)};
            [[NSJSONSerialization dataWithJSONObject:state options:0 error:nil]
                writeToFile:[NSString stringWithUTF8String:argv[1]] atomically:YES];
            printf("READY %d\n", getpid());
            fflush(stdout);
        });
        [app run];
    }
    return 0;
}
