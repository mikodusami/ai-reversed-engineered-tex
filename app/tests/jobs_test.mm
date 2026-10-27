// Headless tests for the Jobs window's review rendering.
// RTEX_RENDER_PNG=path also writes an image of the rendered review.
#import <Cocoa/Cocoa.h>
#import "RTJobs.h"

static int failures = 0;
static void Check(BOOL ok, const char *what) {
    if (!ok) failures++;
    printf("%s %s\n", ok ? "ok  " : "FAIL", what);
}

int main(int argc, const char **argv) {
    @autoreleasepool {
        [NSApplication sharedApplication];
        NSString *md = argc > 1 ? [NSString stringWithContentsOfFile:@(argv[1]) encoding:NSUTF8StringEncoding error:nil] : nil;
        if (!md) {
            md = @"# Acme - Engineer\n\n## Analysis\n\n**Bold** and `code`.\n\n| Keyword | Status |\n|---|---|\n| React | added |\n| C# | gap |\n\n"
                 @"- Check this claim: Skills now lists `Node.js`\n```latex\n\\item x\n```\n";
        }
        NSAttributedString *a = [RTJobsController renderMarkdown:md];
        __block NSUInteger tableCells = 0, bold = 0, mono = 0, orange = 0;
        [a enumerateAttributesInRange:NSMakeRange(0, a.length) options:0 usingBlock:^(NSDictionary *attrs, NSRange r, BOOL *stop) {
            NSParagraphStyle *ps = attrs[NSParagraphStyleAttributeName];
            if ([ps.textBlocks.firstObject isKindOfClass:NSTextTableBlock.class]) tableCells++;
            NSFont *f = attrs[NSFontAttributeName];
            if (f.fontDescriptor.symbolicTraits & NSFontDescriptorTraitMonoSpace) mono++;
            if ([[f.fontDescriptor objectForKey:NSFontTraitsAttribute][NSFontWeightTrait] doubleValue] > 0.2) bold++;
            if ([attrs[NSForegroundColorAttributeName] isEqual:NSColor.systemOrangeColor]) orange++;
        }];
        Check(tableCells >= 6, "markdown tables become NSTextTable cells");
        Check(bold > 0, "headings and **bold** are bold");
        Check(mono > 0, "code is monospaced");
        Check(orange > 0, "claims to check are highlighted");
        Check(![a.string containsString:@"|---"], "table separator rows are not shown");

        const char *png = getenv("RTEX_RENDER_PNG");
        if (png) {
            NSTextView *tv = [[NSTextView alloc] initWithFrame:NSMakeRect(0, 0, 900, 10)];
            tv.textContainerInset = NSMakeSize(18, 14);
            tv.verticallyResizable = YES;
            tv.appearance = [NSAppearance appearanceNamed:NSAppearanceNameDarkAqua];
            tv.backgroundColor = NSColor.textBackgroundColor;
            [tv.textStorage setAttributedString:a];
            [tv.layoutManager ensureLayoutForTextContainer:tv.textContainer];
            NSRect used = [tv.layoutManager usedRectForTextContainer:tv.textContainer];
            [tv setFrameSize:NSMakeSize(900, MIN(used.size.height + 40, 2400))];
            NSBitmapImageRep *rep = [tv bitmapImageRepForCachingDisplayInRect:tv.bounds];
            [tv.appearance performAsCurrentDrawingAppearance:^{ [tv cacheDisplayInRect:tv.bounds toBitmapImageRep:rep]; }];
            [[rep representationUsingType:NSBitmapImageFileTypePNG properties:@{}] writeToFile:@(png) atomically:YES];
        }
    }
    printf("%s\n", failures ? "FAILED" : "all passed");
    return failures ? 1 : 0;
}
