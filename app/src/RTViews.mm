#import "RTViews.h"
#include "RTCore.h"
#include <algorithm>
#include <vector>

// MARK: - Text view

@implementation RTTextView {
    std::vector<NSUInteger> _lineStarts;
    BOOL _linesValid;
    // Command-completion cycle state.
    NSArray<NSDictionary *> *_ccCandidates;
    NSString *_ccWord;
    NSUInteger _ccStart, _ccLength;
    NSInteger _ccIndex;
    NSRange _ccSelection;
    NSUInteger _editCount, _ccEditCount;
}

- (instancetype)initWithFrame:(NSRect)frame textContainer:(NSTextContainer *)container {
    if ((self = [super initWithFrame:frame textContainer:container])) {
        [NSNotificationCenter.defaultCenter addObserver:self
                                               selector:@selector(storageChanged:)
                                                   name:NSTextStorageDidProcessEditingNotification
                                                 object:self.textStorage];
    }
    return self;
}

- (void)dealloc {
    [NSNotificationCenter.defaultCenter removeObserver:self];
}

- (void)storageChanged:(NSNotification *)note {
    _linesValid = NO;
}

- (void)buildLineStarts {
    if (_linesValid) return;
    _lineStarts.assign(1, 0);
    NSString *s = self.string;
    NSUInteger n = s.length;
    unichar buf[8192];
    for (NSUInteger off = 0; off < n; off += 8192) {
        NSUInteger len = MIN((NSUInteger)8192, n - off);
        [s getCharacters:buf range:NSMakeRange(off, len)];
        for (NSUInteger i = 0; i < len; i++) {
            if (buf[i] == '\n') _lineStarts.push_back(off + i + 1);
        }
    }
    _linesValid = YES;
}

- (NSUInteger)lineForIndex:(NSUInteger)index {
    [self buildLineStarts];
    auto it = std::upper_bound(_lineStarts.begin(), _lineStarts.end(), index);
    return (NSUInteger)(it - _lineStarts.begin());
}

- (NSUInteger)indexForLine:(NSUInteger)line {
    [self buildLineStarts];
    if (line == 0) line = 1;
    if (line > _lineStarts.size()) return self.string.length;
    return _lineStarts[line - 1];
}

- (NSUInteger)lineCount {
    [self buildLineStarts];
    return _lineStarts.size();
}

- (void)selectLine:(NSUInteger)line {
    NSUInteger start = [self indexForLine:line];
    NSRange r = [self.string lineRangeForRange:NSMakeRange(MIN(start, self.string.length), 0)];
    // Do not select the trailing newline.
    if (r.length > 0 && [self.string characterAtIndex:NSMaxRange(r) - 1] == '\n') r.length--;
    [self.window makeFirstResponder:self];
    [self setSelectedRange:r];
    [self scrollRangeToVisible:r];
    [self showFindIndicatorForRange:r];
}

- (void)mouseDown:(NSEvent *)event {
    if ((event.modifierFlags & NSEventModifierFlagCommand) && self.syncDelegate) {
        NSPoint p = [self convertPoint:event.locationInWindow fromView:nil];
        NSUInteger idx = [self characterIndexForInsertionAtPoint:p];
        [self.syncDelegate syncFromSourceLine:[self lineForIndex:idx]];
        return;
    }
    [super mouseDown:event];
}

// Apply `transform` to every line touched by the selection, as one undo step.
- (void)transformSelectedLines:(NSString * (^)(NSArray<NSString *> *lines))transform {
    NSString *s = self.string;
    NSRange lineRange = [s lineRangeForRange:self.selectedRange];
    NSString *block = [s substringWithRange:lineRange];
    BOOL trailingNewline = [block hasSuffix:@"\n"];
    if (trailingNewline) block = [block substringToIndex:block.length - 1];
    NSString *replacement = transform([block componentsSeparatedByString:@"\n"]);
    if (trailingNewline) replacement = [replacement stringByAppendingString:@"\n"];
    if (![self shouldChangeTextInRange:lineRange replacementString:replacement]) return;
    [self.textStorage replaceCharactersInRange:lineRange withString:replacement];
    [self didChangeText];
    [self setSelectedRange:NSMakeRange(lineRange.location, replacement.length - (trailingNewline ? 1 : 0))];
}

// TeXShop's Comment / Uncomment: `%` at the start of each line.
- (IBAction)toggleComment:(id)sender {
    [self transformSelectedLines:^NSString *(NSArray<NSString *> *lines) {
        BOOL allCommented = YES;
        for (NSString *l in lines) {
            if (l.length && ![l hasPrefix:@"%"]) { allCommented = NO; break; }
        }
        NSMutableArray *out = [NSMutableArray arrayWithCapacity:lines.count];
        for (NSString *l in lines) {
            if (allCommented) [out addObject:[l hasPrefix:@"%"] ? [l substringFromIndex:1] : l];
            else [out addObject:[@"%" stringByAppendingString:l]];
        }
        return [out componentsJoinedByString:@"\n"];
    }];
}

- (IBAction)shiftRight:(id)sender {
    [self transformSelectedLines:^NSString *(NSArray<NSString *> *lines) {
        NSMutableArray *out = [NSMutableArray arrayWithCapacity:lines.count];
        for (NSString *l in lines) [out addObject:l.length ? [@"\t" stringByAppendingString:l] : l];
        return [out componentsJoinedByString:@"\n"];
    }];
}

- (IBAction)shiftLeft:(id)sender {
    [self transformSelectedLines:^NSString *(NSArray<NSString *> *lines) {
        NSMutableArray *out = [NSMutableArray arrayWithCapacity:lines.count];
        for (NSString *l in lines) {
            if ([l hasPrefix:@"\t"]) { [out addObject:[l substringFromIndex:1]]; continue; }
            NSUInteger n = 0;
            while (n < 4 && n < l.length && [l characterAtIndex:n] == ' ') n++;
            [out addObject:[l substringFromIndex:n]];
        }
        return [out componentsJoinedByString:@"\n"];
    }];
}


// MARK: Command completion

- (void)didChangeText {
    [super didChangeText];
    _editCount++;
}

static BOOL IsBoundary(unichar c) {
    switch (c) {
        case ' ': case '\t': case '\n': case '\r': case '.': case ',': case ';': case ':':
        case '{': case '}': case '(': case ')': case '\\':
            return YES;
        default:
            return NO;
    }
}

// MARK: Auto-pairing (TeXShop's autocompletion.plist + RTeX pairs)

static NSString *const RTAutoClosedKey = @"RTAutoClosed";
NSString *const RTDefaultsAutoPair = @"RTAutoPair";

// Pair table: the first autocompletion.plist found (RTeX, then TeXShop's user
// copy, then TeXShop's bundled one), plus RTeX's `{ [ (` pairs where missing.
static NSDictionary<NSString *, NSString *> *PairTable(void) {
    static NSDictionary *table;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        NSMutableDictionary *t = [NSMutableDictionary dictionary];
        NSDictionary *defaults = rt::json(rtex_autopair_defaults());
        if ([defaults isKindOfClass:NSDictionary.class]) [t addEntriesFromDictionary:defaults];
        NSArray *paths = @[
            [NSHomeDirectory() stringByAppendingPathComponent:@"Library/RTeX/autocompletion.plist"],
            [NSHomeDirectory() stringByAppendingPathComponent:@"Library/TeXShop/Keyboard/autocompletion.plist"],
            @"/Applications/TeX/TeXShop.app/Contents/Resources/TeXShop/Keyboard/autocompletion.plist",
        ];
        for (NSString *p in paths) {
            NSDictionary *d = [NSDictionary dictionaryWithContentsOfFile:p];
            if (d.count) {
                [t addEntriesFromDictionary:d];
                break;
            }
        }
        table = t;
    });
    return table;
}

static unichar CloserFor(unichar c) {
    switch (c) {
        case '{': return '}';
        case '[': return ']';
        case '(': return ')';
        default: return 0;
    }
}

- (BOOL)autoPairEnabled {
    return [NSUserDefaults.standardUserDefaults boolForKey:RTDefaultsAutoPair];
}

- (BOOL)isAutoClosedAt:(NSUInteger)i {
    if (i >= self.string.length) return NO;
    return [self.layoutManager temporaryAttribute:RTAutoClosedKey atCharacterIndex:i effectiveRange:NULL] != nil;
}

- (void)replaceRange:(NSRange)r with:(NSString *)text caret:(NSUInteger)caret actionName:(NSString *)name {
    if (![self shouldChangeTextInRange:r replacementString:text]) return;
    [self.textStorage replaceCharactersInRange:r withString:text];
    [self didChangeText];
    [self.undoManager setActionName:name];
    [self setSelectedRange:NSMakeRange(r.location + caret, 0)];
}

- (void)insertText:(id)insert replacementRange:(NSRange)replacement {
    NSString *str = [insert isKindOfClass:NSAttributedString.class] ? [insert string] : insert;
    if (![self autoPairEnabled] || ![str isKindOfClass:NSString.class] || str.length != 1 || self.hasMarkedText ||
        replacement.location != NSNotFound) {
        [super insertText:insert replacementRange:replacement];
        return;
    }
    NSString *s = self.string;
    NSRange sel = self.selectedRange;
    unichar c = [str characterAtIndex:0];

    // Type over a closer we inserted ourselves.
    if (sel.length == 0 && (c == '}' || c == ']' || c == ')') && sel.location < s.length &&
        [s characterAtIndex:sel.location] == c && [self isAutoClosedAt:sel.location]) {
        [self.layoutManager removeTemporaryAttribute:RTAutoClosedKey forCharacterRange:NSMakeRange(sel.location, 1)];
        [self setSelectedRange:NSMakeRange(sel.location + 1, 0)];
        return;
    }
    // TeXShop: ASCII keys right after a backslash are literal (\{, \^, \_).
    if (c < 0x80 && sel.location > 0 && [s characterAtIndex:sel.location - 1] == '\\') {
        [super insertText:insert replacementRange:replacement];
        return;
    }
    NSString *tmpl = PairTable()[str];
    if (!tmpl && c == '$' && sel.length > 0) tmpl = @"$#SEL##INS#$";  // wrap a selection in math
    if (!tmpl) {
        [super insertText:insert replacementRange:replacement];
        return;
    }
    NSString *selected = [s substringWithRange:sel];
    NSDictionary *r = rt::dict(rtex_autopair_expand(tmpl.UTF8String, selected.UTF8String));
    NSString *text = r[@"text"];
    if (!text) {
        [super insertText:insert replacementRange:replacement];
        return;
    }
    [self replaceRange:sel with:text caret:[r[@"caret"] unsignedIntegerValue] actionName:@"Autocompletion"];
    // Remember a closer we put right after the caret (`{|}`, `^{|}`) so it
    // can be typed over or removed with Backspace.
    NSUInteger caret = [r[@"caret"] unsignedIntegerValue];
    if (sel.length == 0 && caret + 1 == text.length) {
        unichar last = [text characterAtIndex:caret];
        if (last == '}' || last == ']' || last == ')') {
            [self.layoutManager addTemporaryAttribute:RTAutoClosedKey value:@YES
                                    forCharacterRange:NSMakeRange(sel.location + caret, 1)];
        }
    }
}

// Backspace between an opener and the closer we added removes both.
- (void)deleteBackward:(id)sender {
    NSRange sel = self.selectedRange;
    NSString *s = self.string;
    if ([self autoPairEnabled] && sel.length == 0 && sel.location > 0 && sel.location < s.length) {
        unichar close = CloserFor([s characterAtIndex:sel.location - 1]);
        if (close && [s characterAtIndex:sel.location] == close && [self isAutoClosedAt:sel.location]) {
            [self replaceRange:NSMakeRange(sel.location - 1, 2) with:@"" caret:0 actionName:@"Delete"];
            return;
        }
    }
    [super deleteBackward:sender];
}

// TeXShop's Esc after `\begin{name}`: add the matching `\end{name}` below.
- (BOOL)insertEndEnvironment {
    NSString *s = self.string;
    NSRange sel = self.selectedRange;
    if (sel.length || sel.location == 0 || [s characterAtIndex:sel.location - 1] != '}') return NO;
    NSRange line = [s lineRangeForRange:NSMakeRange(sel.location, 0)];
    NSString *before = [s substringWithRange:NSMakeRange(line.location, sel.location - line.location)];
    NSUInteger i = 0;
    while (i < before.length && ([before characterAtIndex:i] == ' ' || [before characterAtIndex:i] == '\t')) i++;
    NSString *indent = [before substringToIndex:i];
    char *raw = rtex_end_environment(before.UTF8String, indent.UTF8String);
    if (!raw) return NO;
    NSString *text = [NSString stringWithUTF8String:raw];
    rtex_string_free(raw);
    // Already closed right below? Then do nothing (a second Esc).
    NSString *after = [s substringFromIndex:sel.location];
    if ([after hasPrefix:[text substringToIndex:text.length - 1]]) {
        NSBeep();
        return YES;
    }
    [self replaceRange:sel with:text caret:0 actionName:@"Completion"];
    return YES;
}

// Esc arrives as cancelOperation:, which NSTextView does not forward to
// complete: on current macOS. Route it ourselves (TeXShop's trigger key).
- (void)cancelOperation:(id)sender {
    [self complete:sender];
}

// Esc / complete:, as in TeXShop: complete the
// word left of the cursor; pressing again cycles, ending back at the word.
- (void)complete:(id)sender {
    NSRange sel = self.selectedRange;
    if (_ccCandidates && _editCount == _ccEditCount && NSEqualRanges(sel, _ccSelection)) {
        _ccIndex = (_ccIndex + 1) % (NSInteger)(_ccCandidates.count + 1);
        [self applyCompletion];
        return;
    }
    _ccCandidates = nil;
    if ([self insertEndEnvironment]) return;
    NSString *s = self.string;
    NSUInteger end = sel.location, start = end;
    // A selected Mark is replaced; complete the word before it.
    while (start > 0 && !IsBoundary([s characterAtIndex:start - 1])) start--;
    if (start > 0) {
        unichar b = [s characterAtIndex:start - 1];
        if (b == '\\' || b == '{') start--;
    }
    if (start == end) {
        NSBeep();
        return;
    }
    NSString *word = [s substringWithRange:NSMakeRange(start, end - start)];
    NSRange line = [s lineRangeForRange:NSMakeRange(start, 0)];
    NSUInteger i = line.location;
    while (i < start && ([s characterAtIndex:i] == ' ' || [s characterAtIndex:i] == '\t')) i++;
    NSString *indent = [s substringWithRange:NSMakeRange(line.location, i - line.location)];
    NSDictionary *r = rt::dict(rtex_complete(word.UTF8String, indent.UTF8String));
    NSArray *cands = r[@"candidates"];
    if (![cands isKindOfClass:NSArray.class] || !cands.count) {
        NSBeep();
        return;
    }
    _ccCandidates = cands;
    _ccWord = word;
    _ccStart = start;
    _ccLength = NSMaxRange(sel) - start;
    _ccIndex = 0;
    [self applyCompletion];
}

- (void)applyCompletion {
    NSString *text;
    NSRange select;
    if ((NSUInteger)_ccIndex == _ccCandidates.count) {  // back to what was typed
        text = _ccWord;
        select = NSMakeRange(_ccStart + text.length, 0);
    } else {
        NSDictionary *c = _ccCandidates[_ccIndex];
        text = c[@"text"];
        select = NSMakeRange(_ccStart + [c[@"sel_start"] unsignedIntegerValue], [c[@"sel_len"] unsignedIntegerValue]);
    }
    NSRange replace = NSMakeRange(_ccStart, _ccLength);
    if (![self shouldChangeTextInRange:replace replacementString:text]) return;
    [self.textStorage replaceCharactersInRange:replace withString:text];
    [self didChangeText];
    _ccLength = text.length;
    [self setSelectedRange:select];
    [self scrollRangeToVisible:select];
    _ccSelection = select;
    _ccEditCount = _editCount;
}

// MARK: Marks (`•` and `•‹comment›`)

static NSRegularExpression *MarkRegex(void) {
    static NSRegularExpression *re;
    static dispatch_once_t once;
    dispatch_once(&once, ^{ re = [NSRegularExpression regularExpressionWithPattern:@"•‹[^›\\n]*›|•" options:0 error:nil]; });
    return re;
}

- (void)findMarkForward:(BOOL)forward deleting:(BOOL)del {
    NSString *s = self.string;
    NSRange sel = self.selectedRange;
    NSRange search = forward ? NSMakeRange(NSMaxRange(sel), s.length - NSMaxRange(sel)) : NSMakeRange(0, sel.location);
    NSArray<NSTextCheckingResult *> *hits = [MarkRegex() matchesInString:s options:0 range:search];
    NSTextCheckingResult *hit = forward ? hits.firstObject : hits.lastObject;
    if (!hit) {
        NSBeep();
        return;
    }
    NSRange r = hit.range;
    if (del) {
        if (![self shouldChangeTextInRange:r replacementString:@""]) return;
        [self.textStorage replaceCharactersInRange:r withString:@""];
        [self didChangeText];
        r.length = 0;
    }
    [self setSelectedRange:r];
    [self scrollRangeToVisible:r];
}

- (IBAction)nextMark:(id)sender { [self findMarkForward:YES deleting:NO]; }
- (IBAction)previousMark:(id)sender { [self findMarkForward:NO deleting:NO]; }
- (IBAction)nextMarkDelete:(id)sender { [self findMarkForward:YES deleting:YES]; }
- (IBAction)previousMarkDelete:(id)sender { [self findMarkForward:NO deleting:YES]; }

- (IBAction)insertMark:(id)sender {
    [self insertText:@"•" replacementRange:self.selectedRange];
}

- (IBAction)insertMarkComment:(id)sender {
    NSUInteger at = self.selectedRange.location;
    [self insertText:@"•‹›" replacementRange:self.selectedRange];
    [self setSelectedRange:NSMakeRange(at + 2, 0)];
}

@end

// MARK: - Line numbers

@implementation RTLineNumberRuler {
    __weak NSTextView *_textView;
}

- (instancetype)initWithTextView:(NSTextView *)textView {
    if ((self = [super initWithScrollView:textView.enclosingScrollView orientation:NSVerticalRuler])) {
        _textView = textView;
        self.clientView = textView;
        self.ruleThickness = 44;
        NSView *clip = textView.enclosingScrollView.contentView;
        clip.postsBoundsChangedNotifications = YES;
        [NSNotificationCenter.defaultCenter addObserver:self
                                               selector:@selector(refresh:)
                                                   name:NSViewBoundsDidChangeNotification
                                                 object:clip];
        [NSNotificationCenter.defaultCenter addObserver:self
                                               selector:@selector(refresh:)
                                                   name:NSTextStorageDidProcessEditingNotification
                                                 object:textView.textStorage];
    }
    return self;
}

- (void)dealloc {
    [NSNotificationCenter.defaultCenter removeObserver:self];
}

- (void)refresh:(NSNotification *)n {
    // Defer until layout has caught up with the edit.
    dispatch_async(dispatch_get_main_queue(), ^{ [self setNeedsDisplay:YES]; });
}

- (void)invalidateLines {
    [self setNeedsDisplay:YES];
}

- (void)drawHashMarksAndLabelsInRect:(NSRect)rect {
    RTTextView *tv = (RTTextView *)_textView;
    if (!tv) return;
    [NSColor.controlBackgroundColor setFill];
    NSRectFill(self.bounds);
    [NSColor.separatorColor setFill];
    NSRectFill(NSMakeRect(NSMaxX(self.bounds) - 1, NSMinY(rect), 1, NSHeight(rect)));

    NSLayoutManager *lm = tv.layoutManager;
    NSTextContainer *tc = tv.textContainer;
    NSRect visible = tv.enclosingScrollView.contentView.bounds;
    NSRange glyphs = [lm glyphRangeForBoundingRect:visible inTextContainer:tc];
    NSRange chars = [lm characterRangeForGlyphRange:glyphs actualGlyphRange:NULL];
    NSDictionary *attrs = @{
        NSFontAttributeName : [NSFont monospacedDigitSystemFontOfSize:10 weight:NSFontWeightRegular],
        NSForegroundColorAttributeName : NSColor.secondaryLabelColor,
    };
    NSString *s = tv.string;
    NSUInteger line = [tv lineForIndex:chars.location];
    NSUInteger total = [tv lineCount];
    CGFloat originY = tv.textContainerOrigin.y;
    for (; line <= total; line++) {
        NSUInteger start = [(id)tv indexForLine:line];
        if (start > NSMaxRange(chars)) break;
        NSRect frag;
        if (start >= s.length) {
            if (start > 0 && s.length && [s characterAtIndex:s.length - 1] != '\n') break;
            frag = lm.extraLineFragmentRect;
            if (NSIsEmptyRect(frag) && s.length) break;
        } else {
            NSUInteger g = [lm glyphIndexForCharacterAtIndex:start];
            frag = [lm lineFragmentRectForGlyphAtIndex:g effectiveRange:NULL];
        }
        NSPoint p = [self convertPoint:NSMakePoint(0, NSMinY(frag) + originY) fromView:tv];
        NSString *label = [NSString stringWithFormat:@"%lu", (unsigned long)line];
        NSSize size = [label sizeWithAttributes:attrs];
        CGFloat y = p.y + (NSHeight(frag) - size.height) / 2;
        [label drawAtPoint:NSMakePoint(self.ruleThickness - size.width - 6, y) withAttributes:attrs];
    }
}

@end

// MARK: - PDF view

@implementation RTPDFView

- (void)mouseDown:(NSEvent *)event {
    if ((event.modifierFlags & NSEventModifierFlagCommand) && self.syncDelegate && self.document) {
        NSPoint viewPoint = [self convertPoint:event.locationInWindow fromView:nil];
        PDFPage *page = [self pageForPoint:viewPoint nearest:YES];
        if (page) {
            NSPoint pp = [self convertPoint:viewPoint toPage:page];
            NSRect media = [page boundsForBox:kPDFDisplayBoxMediaBox];
            CGFloat x = pp.x - media.origin.x;
            CGFloat y = NSMaxY(media) - pp.y;  // top-left origin, like SyncTeX
            NSUInteger index = [self.document indexForPage:page];
            [self.syncDelegate syncFromPDFPage:index + 1 x:x y:y];
            return;
        }
    }
    [super mouseDown:event];
}

- (void)showRects:(NSArray<NSValue *> *)rects onPage:(NSUInteger)pageNumber {
    if (!self.document || pageNumber == 0 || pageNumber > self.document.pageCount) return;
    PDFPage *page = [self.document pageAtIndex:pageNumber - 1];
    NSRect media = [page boundsForBox:kPDFDisplayBoxMediaBox];
    NSMutableArray<PDFAnnotation *> *marks = [NSMutableArray array];
    NSRect all = NSZeroRect;
    for (NSValue *v in rects) {
        NSRect r = v.rectValue;
        CGFloat h = MAX(r.size.height, 8);
        NSRect pr = NSMakeRect(media.origin.x + r.origin.x - 2, NSMaxY(media) - r.origin.y - h - 1,
                               MAX(r.size.width, 8) + 4, h + 2);
        PDFAnnotation *a = [[PDFAnnotation alloc] initWithBounds:pr
                                                         forType:PDFAnnotationSubtypeSquare
                                                  withProperties:nil];
        PDFBorder *border = [[PDFBorder alloc] init];
        border.lineWidth = 0;
        a.border = border;
        a.color = NSColor.clearColor;
        a.interiorColor = [NSColor.systemYellowColor colorWithAlphaComponent:0.45];
        [page addAnnotation:a];
        [marks addObject:a];
        all = NSIsEmptyRect(all) ? pr : NSUnionRect(all, pr);
    }
    if (!marks.count) return;
    [self goToRect:NSInsetRect(all, 0, -60) onPage:page];
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)(1.6 * NSEC_PER_SEC)), dispatch_get_main_queue(), ^{
        for (PDFAnnotation *a in marks) [page removeAnnotation:a];
    });
}

@end
