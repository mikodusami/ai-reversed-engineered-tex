// Headless tests for RTDocument saving (TeXShop-compatible behaviour):
// encoding line at save time, no silent character loss, extension kept,
// reloading on outside changes, keeping edits, save-related files.
#import <Cocoa/Cocoa.h>
#import "RTDocument.h"

@interface RTDocument (Testing)
- (void)saveRelatedDocumentsThen:(void (^)(void))next;
- (NSString *)currentText;
@end

static int failures = 0;
static void Check(BOOL ok, const char *what, NSString *detail) {
    if (!ok) failures++;
    printf("%s %-58s %s\n", ok ? "ok  " : "FAIL", what, ok ? "" : detail.UTF8String);
}

static void Spin(double seconds) {
    [NSRunLoop.mainRunLoop runUntilDate:[NSDate dateWithTimeIntervalSinceNow:seconds]];
}

static RTDocument *Open(NSString *path) {
    NSError *e = nil;
    RTDocument *d = [[RTDocument alloc] initWithContentsOfURL:[NSURL fileURLWithPath:path] ofType:@"LaTeX Document" error:&e];
    if (!d) printf("open failed: %s\n", e.description.UTF8String);
    [NSDocumentController.sharedDocumentController addDocument:d];
    [d makeWindowControllers];
    return d;
}

static NSTextView *TextView(RTDocument *d) {
    return (NSTextView *)d.windowControllers.firstObject.window.initialFirstResponder;
}

static void Type(RTDocument *d, NSString *s) {
    NSTextView *tv = TextView(d);
    [tv setSelectedRange:NSMakeRange(tv.string.length, 0)];
    [tv insertText:s replacementRange:NSMakeRange(NSNotFound, 0)];
    Spin(0.05);  // let the undo group close so the document counts the edit
}

static BOOL Save(RTDocument *d, NSError **err) {
    __block BOOL done = NO;
    __block NSError *result = nil;
    [d saveToURL:d.fileURL ofType:d.fileType forSaveOperation:NSSaveOperation completionHandler:^(NSError *e) {
        result = e;
        done = YES;
    }];
    while (!done) Spin(0.02);
    if (err) *err = result;
    return result == nil;
}

int main() {
    @autoreleasepool {
        [NSApplication sharedApplication];
        NSUserDefaults *ud = NSUserDefaults.standardUserDefaults;
        for (NSString *k in @[ RTDefaultsWatchFiles, RTDefaultsSaveRelated ]) [ud setBool:YES forKey:k];
        for (NSString *k in @[ RTDefaultsTypesetOnSave, RTDefaultsPrecomposeUnicode, RTDefaultsKeepBackup ]) [ud setBool:NO forKey:k];

        NSString *dir = [NSTemporaryDirectory() stringByAppendingPathComponent:
                                                    [NSString stringWithFormat:@"rtex-save-%d", getpid()]];
        [NSFileManager.defaultManager createDirectoryAtPath:dir withIntermediateDirectories:YES attributes:nil error:nil];
        NSString *(^P)(NSString *) = ^(NSString *n) { return [dir stringByAppendingPathComponent:n]; };
        NSError *err = nil;

        // 1. Encoding line decides the encoding at save time.
        [@"% !TEX encoding = UTF-8 Unicode\nCaf\u00e9\n" writeToFile:P(@"enc.tex") atomically:YES encoding:NSUTF8StringEncoding error:nil];
        RTDocument *d = Open(P(@"enc.tex"));
        NSTextView *tv = TextView(d);
        [tv setSelectedRange:NSMakeRange(17, 13)];
        [tv insertText:@"IsoLatin" replacementRange:NSMakeRange(NSNotFound, 0)];
        Spin(0.05);
        Save(d, &err);
        NSData *bytes = [NSData dataWithContentsOfFile:P(@"enc.tex")];
        const unsigned char *b = (const unsigned char *)bytes.bytes;
        Check(bytes.length > 0 && b[bytes.length - 2] == 0xE9, "% !TEX encoding changed to Latin-1 -> saved as Latin-1",
              [NSString stringWithFormat:@"%@", bytes]);

        // 2. A character the declared encoding can't hold: refuse, don't drop it.
        Type(d, @"\U0001F600");
        BOOL ok = Save(d, &err);
        Check(!ok && err.code == NSFileWriteInapplicableStringEncodingError, "emoji in Latin-1 file -> save refused with reason",
              err.localizedDescription ?: @"saved");
        NSString *onDisk = [NSString stringWithContentsOfFile:P(@"enc.tex") encoding:NSISOLatin1StringEncoding error:nil];
        Check(![onDisk containsString:@"?"] && [d.currentText containsString:@"\U0001F600"], "file and text left intact", onDisk);
        [d close];

        // 3. No encoding line: a Latin-1 file that gains an emoji becomes UTF-8.
        NSData *latin = [@"Caf\u00e9\n" dataUsingEncoding:NSISOLatin1StringEncoding];
        [latin writeToFile:P(@"plain.tex") atomically:YES];
        d = Open(P(@"plain.tex"));
        Type(d, @"\U0001F600");
        ok = Save(d, &err);
        NSString *utf8 = [NSString stringWithContentsOfFile:P(@"plain.tex") encoding:NSUTF8StringEncoding error:nil];
        Check(ok && [utf8 containsString:@"Caf\u00e9"] && [utf8 containsString:@"\U0001F600"],
              "no encoding line + emoji -> saved as UTF-8, nothing lost", utf8 ?: @"(not UTF-8)");
        [d close];

        // 4. Extension is kept.
        [@"\\ProvidesPackage{x}\n" writeToFile:P(@"x.sty") atomically:YES encoding:NSUTF8StringEncoding error:nil];
        d = Open(P(@"x.sty"));
        Check([[d fileNameExtensionForType:d.fileType saveOperation:NSSaveOperation] isEqual:@"sty"], ".sty keeps its extension", @"");
        [d close];

        // 5. Outside write -> reload (no unsaved edits).
        [@"one\n" writeToFile:P(@"watch.tex") atomically:NO encoding:NSUTF8StringEncoding error:nil];
        d = Open(P(@"watch.tex"));
        Spin(0.2);
        [@"two\n" writeToFile:P(@"watch.tex") atomically:NO encoding:NSUTF8StringEncoding error:nil];
        Spin(0.5);
        Check([d.currentText isEqual:@"two\n"], "other app writes the file -> reloaded", d.currentText);

        // 6. Outside save-by-replace (atomic rename) -> reloaded after the 1 s re-check.
        [@"three\n" writeToFile:P(@"watch.tex") atomically:YES encoding:NSUTF8StringEncoding error:nil];
        Spin(1.6);
        Check([d.currentText isEqual:@"three\n"], "other app replaces the file -> reloaded", d.currentText);
        [@"four\n" writeToFile:P(@"watch.tex") atomically:NO encoding:NSUTF8StringEncoding error:nil];
        Spin(0.5);
        Check([d.currentText isEqual:@"four\n"], "still watching the new file afterwards", d.currentText);

        // 7. Our own save is not mistaken for an outside change.
        Type(d, @"mine\n");
        Save(d, nil);
        Spin(1.3);
        Check([d.currentText isEqual:@"four\nmine\n"] && !d.isDocumentEdited, "own save: no reload, not edited", d.currentText);

        // 8. Unsaved edits are never overwritten by an outside change.
        Type(d, @"unsaved");
        [@"outside\n" writeToFile:P(@"watch.tex") atomically:NO encoding:NSUTF8StringEncoding error:nil];
        Spin(0.5);
        Check([d.currentText hasSuffix:@"unsaved"], "outside change while edited -> edits kept", d.currentText);
        [d close];

        // 9. File deleted -> window stays, text kept, marked unsaved.
        [@"keep me\n" writeToFile:P(@"gone.tex") atomically:NO encoding:NSUTF8StringEncoding error:nil];
        d = Open(P(@"gone.tex"));
        Spin(0.2);
        [NSFileManager.defaultManager removeItemAtPath:P(@"gone.tex") error:nil];
        Spin(1.6);
        Check([d.currentText isEqual:@"keep me\n"] && d.isDocumentEdited, "file deleted -> text kept, marked unsaved", d.currentText);
        Save(d, nil);
        Check([NSFileManager.defaultManager fileExistsAtPath:P(@"gone.tex")], "saving writes it back", @"");
        [d close];

        // 10. Save related: typesetting main.tex saves an edited \input'ed chapter first.
        [@"\\documentclass{article}\\begin{document}\\input{ch}\\end{document}\n" writeToFile:P(@"main.tex") atomically:YES encoding:NSUTF8StringEncoding error:nil];
        [@"Chapter\n" writeToFile:P(@"ch.tex") atomically:YES encoding:NSUTF8StringEncoding error:nil];
        [@"Other\n" writeToFile:P(@"other.tex") atomically:YES encoding:NSUTF8StringEncoding error:nil];
        RTDocument *mainDoc = Open(P(@"main.tex")), *ch = Open(P(@"ch.tex")), *other = Open(P(@"other.tex"));
        Type(ch, @"edited\n");
        Type(other, @"edited\n");
        __block BOOL done = NO;
        [mainDoc saveRelatedDocumentsThen:^{ done = YES; }];
        while (!done) Spin(0.02);
        NSString *chDisk = [NSString stringWithContentsOfFile:P(@"ch.tex") encoding:NSUTF8StringEncoding error:nil];
        NSString *otherDisk = [NSString stringWithContentsOfFile:P(@"other.tex") encoding:NSUTF8StringEncoding error:nil];
        Check([chDisk hasSuffix:@"edited\n"] && !ch.isDocumentEdited, "\\input'ed chapter saved before typesetting", chDisk);
        Check(![otherDisk containsString:@"edited"] && other.isDocumentEdited, "unrelated open file left alone", otherDisk);
        for (RTDocument *x in @[ mainDoc, ch, other ]) {
            [x updateChangeCount:NSChangeCleared];
            [x close];
        }

        [NSFileManager.defaultManager removeItemAtPath:dir error:nil];
    }
    printf("%s\n", failures ? "FAILED" : "all passed");
    return failures ? 1 : 0;
}
