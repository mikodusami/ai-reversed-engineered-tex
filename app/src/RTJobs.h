// The Jobs window: postings → tailored resumes (rtex-jobs).
#import <Cocoa/Cocoa.h>

extern NSString *const RTDefaultsJobsRoot;
extern NSString *const RTDefaultsJobsAnalysis;  // also write the role analysis (default NO)

@interface RTJobsController : NSWindowController <NSTableViewDataSource, NSTableViewDelegate, NSToolbarDelegate,
                                                  NSWindowDelegate>
+ (instancetype)shared;
- (IBAction)showJobs:(id)sender;
- (IBAction)newJob:(id)sender;
- (IBAction)chooseJobsFolder:(id)sender;
// Markdown → attributed text for review.md (exposed for tests).
+ (NSAttributedString *)renderMarkdown:(NSString *)md;
@end
