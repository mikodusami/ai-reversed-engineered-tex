// Headless tests for RTTextView editing behaviour (auto-pairing, Esc \end,
// command completion). Drives the real view class directly; no keystrokes.
#import <Cocoa/Cocoa.h>
#import "RTViews.h"

static int failures = 0;

static RTTextView *MakeView(NSString *text) {
    NSTextStorage *storage = [[NSTextStorage alloc] initWithString:text];
    NSLayoutManager *lm = [[NSLayoutManager alloc] init];
    [storage addLayoutManager:lm];
    NSTextContainer *tc = [[NSTextContainer alloc] initWithSize:NSMakeSize(600, 1e7)];
    [lm addTextContainer:tc];
    RTTextView *tv = [[RTTextView alloc] initWithFrame:NSMakeRect(0, 0, 600, 400) textContainer:tc];
    tv.richText = NO;
    tv.allowsUndo = YES;
    [tv setSelectedRange:NSMakeRange(text.length, 0)];
    return tv;
}

static void Type(RTTextView *tv, NSString *keys) {
    for (NSUInteger i = 0; i < keys.length; i++) {
        NSRange r = [keys rangeOfComposedCharacterSequenceAtIndex:i];
        [tv insertText:[keys substringWithRange:r] replacementRange:NSMakeRange(NSNotFound, 0)];
        i = NSMaxRange(r) - 1;
    }
}

// Expected text with `|` marking the caret.
static void Expect(RTTextView *tv, NSString *expected, const char *what) {
    NSMutableString *actual = [tv.string mutableCopy];
    [actual insertString:@"|" atIndex:tv.selectedRange.location];
    if (tv.selectedRange.length) [actual insertString:@"⟧" atIndex:NSMaxRange(tv.selectedRange) + 1];
    BOOL ok = [actual isEqualToString:expected];
    if (!ok) failures++;
    printf("%s %-40s %s\n", ok ? "ok  " : "FAIL", what, ok ? "" : [[NSString stringWithFormat:@"got %@ want %@", actual, expected] UTF8String]);
}

int main() {
    @autoreleasepool {
        [NSApplication sharedApplication];
        [NSUserDefaults.standardUserDefaults setBool:YES forKey:RTDefaultsAutoPair];

        RTTextView *tv = MakeView(@"");
        Type(tv, @"x^");
        Expect(tv, @"x^{|}", "^ pairs (TeXShop table)");
        Type(tv, @"2}");
        Expect(tv, @"x^{2}|", "typing } over the auto closer");
        Type(tv, @"+y_");
        Expect(tv, @"x^{2}+y_{|}", "_ pairs");

        tv = MakeView(@"");
        Type(tv, @"\\frac{a}{b}");
        Expect(tv, @"\\frac{a}{b}|", "\\frac{a}{b} typed naturally");

        tv = MakeView(@"");
        Type(tv, @"(");
        Expect(tv, @"(|)", "( pairs");
        [tv deleteBackward:nil];
        Expect(tv, @"|", "backspace removes empty pair");

        tv = MakeView(@"");
        Type(tv, @"\\{");
        Expect(tv, @"\\{|", "\\{ stays literal");

        tv = MakeView(@"a}");
        [tv setSelectedRange:NSMakeRange(1, 0)];
        Type(tv, @"}");
        Expect(tv, @"a}|}", "} not auto-inserted is not typed over");

        tv = MakeView(@"see x+1 here");
        [tv setSelectedRange:NSMakeRange(4, 3)];
        Type(tv, @"$");
        Expect(tv, @"see $x+1|$ here", "$ wraps a selection");
        [tv setSelectedRange:NSMakeRange(5, 3)];
        Type(tv, @"{");
        Expect(tv, @"see ${x+1|}$ here", "{ wraps a selection");

        tv = MakeView(@"");
        Type(tv, @"\"");
        Expect(tv, @"``|''", "\" gives TeX quotes");
        tv = MakeView(@"");
        Type(tv, @"≤");
        Expect(tv, @"\\leq|", "≤ becomes \\leq");

        tv = MakeView(@"  \\begin{itemize}");
        [tv complete:nil];
        Expect(tv, @"  \\begin{itemize}|\n  \\end{itemize}\n", "Esc after \\begin adds \\end");
        [tv complete:nil];
        Expect(tv, @"  \\begin{itemize}|\n  \\end{itemize}\n", "second Esc does not duplicate");

        tv = MakeView(@"\\begin{eq");
        [tv complete:nil];
        Expect(tv, @"\\begin{equation}\n|•⟧\n\\end{equation}•", "Esc completion still works");

        [NSUserDefaults.standardUserDefaults setBool:NO forKey:RTDefaultsAutoPair];
        tv = MakeView(@"");
        Type(tv, @"{");
        Expect(tv, @"{|", "auto-pairing off: plain typing");
        [NSUserDefaults.standardUserDefaults removeObjectForKey:RTDefaultsAutoPair];
    }
    printf("%s\n", failures ? "FAILED" : "all passed");
    return failures ? 1 : 0;
}
