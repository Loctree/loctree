// Minimal Objective-C interface fixture for symbol_graph Wave B extraction.
// Exercises: @interface declaration (Declares edge), #import (Includes edge),
// @property, and an IBAction declaration (IBActionBinding surface).

#import <Foundation/Foundation.h>

// @interface — the declaring half; pairs with @implementation in the .m
// (Declares ↔ Defines across the .h↔.m split).
@interface EditorViewController : NSObject

@property (nonatomic, copy) NSString *documentText;

// Target-action handler declared here, implemented in the .m.
- (IBAction)saveDocument:(id)sender;

- (void)startObserving;

@end
