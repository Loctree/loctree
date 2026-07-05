// Minimal Objective-C implementation fixture for symbol_graph Wave B.
// Exercises: @implementation (Defines, pairs with @interface in the .h),
// #import of the header (Includes edge), @selector (SelectorMessage edge),
// target-action wiring, and an NSNotificationCenter post/observe pair
// (NotificationEmit ↔ NotificationObserve).

#import "EditorViewController.h"

// Notification name constant both the post and observe sites pair on.
static NSString *const VCDocumentChangedNotification = @"VCDocumentChanged";

@implementation EditorViewController

- (void)startObserving {
    // NotificationObserve: addObserver with a @selector target-action.
    [[NSNotificationCenter defaultCenter] addObserver:self
                                             selector:@selector(handleDocumentChanged:)
                                                 name:VCDocumentChangedNotification
                                               object:nil];
}

// IBActionBinding implementation (declared in the .h).
- (IBAction)saveDocument:(id)sender {
    self.documentText = @"saved";
    // NotificationEmit: pairs with the observer above.
    [[NSNotificationCenter defaultCenter] postNotificationName:VCDocumentChangedNotification
                                                        object:self];
}

// SelectorMessage target of @selector(handleDocumentChanged:).
- (void)handleDocumentChanged:(NSNotification *)note {
    NSLog(@"document changed: %@", note.object);
}

@end
