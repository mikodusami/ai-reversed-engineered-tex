// Editor, line-number ruler and PDF views.
#import <Cocoa/Cocoa.h>
#import <PDFKit/PDFKit.h>

extern NSString *const RTDefaultsAutoPair;

@protocol RTSyncDelegate <NSObject>
// Cmd-click in the source: show `line` in the PDF.
- (void)syncFromSourceLine:(NSUInteger)line;
// Cmd-click in the PDF (page 1-based, PDF points from the top-left).
- (void)syncFromPDFPage:(NSUInteger)page x:(CGFloat)x y:(CGFloat)y;
@end

@interface RTTextView : NSTextView
@property(weak) id<RTSyncDelegate> syncDelegate;
// 1-based line number of a character index.
- (NSUInteger)lineForIndex:(NSUInteger)index;
// UTF-16 index of the start of a 1-based line, and the number of lines.
- (NSUInteger)indexForLine:(NSUInteger)line;
- (NSUInteger)lineCount;
// Select (and scroll to) a 1-based line.
- (void)selectLine:(NSUInteger)line;
- (IBAction)toggleComment:(id)sender;
- (IBAction)shiftRight:(id)sender;
- (IBAction)shiftLeft:(id)sender;
// Command completion (Esc) and TeXShop's Marks commands.
- (IBAction)nextMark:(id)sender;
- (IBAction)previousMark:(id)sender;
- (IBAction)nextMarkDelete:(id)sender;
- (IBAction)previousMarkDelete:(id)sender;
- (IBAction)insertMark:(id)sender;
- (IBAction)insertMarkComment:(id)sender;
@end

@interface RTLineNumberRuler : NSRulerView
- (instancetype)initWithTextView:(NSTextView *)textView;
- (void)invalidateLines;
@end

@interface RTPDFView : PDFView
@property(weak) id<RTSyncDelegate> syncDelegate;
// Flash rectangles (PDF points, top-left origin) on a 1-based page.
- (void)showRects:(NSArray<NSValue *> *)rects onPage:(NSUInteger)page;
@end
