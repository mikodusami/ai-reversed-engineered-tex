// One source document: editor + console on the left, PDF preview on the right
// (TeXShop's TSDocument + its preview window, in a single window).
#import <Cocoa/Cocoa.h>
#import "RTViews.h"

extern NSString *const RTDefaultsFastPreamble;
extern NSString *const RTDefaultsSinglePass;
extern NSString *const RTDefaultsTypesetOnSave;
extern NSString *const RTDefaultsShellEscape;
extern NSString *const RTDefaultsAutoSave;
extern NSString *const RTDefaultsKeepBackup;
extern NSString *const RTDefaultsWatchFiles;
extern NSString *const RTDefaultsSaveRelated;
extern NSString *const RTDefaultsEncodingInSavePanel;
extern NSString *const RTDefaultsPrecomposeUnicode;
extern NSString *const RTDefaultsAfterBuild;

@interface RTDocument : NSDocument <NSTextStorageDelegate, NSTextViewDelegate, NSToolbarDelegate,
                                    NSMenuDelegate, NSWindowDelegate, RTSyncDelegate>
- (IBAction)typeset:(id)sender;
- (IBAction)stopTypesetting:(id)sender;
- (IBAction)trashAuxFiles:(id)sender;
- (IBAction)showBuildPanel:(id)sender;
- (IBAction)setAfterBuild:(NSMenuItem *)sender;
- (IBAction)syncToPDF:(id)sender;
- (IBAction)goToNextError:(id)sender;
- (IBAction)toggleComment:(id)sender;
- (IBAction)shiftRight:(id)sender;
- (IBAction)shiftLeft:(id)sender;
- (void)selectLine:(NSUInteger)line;
@end
