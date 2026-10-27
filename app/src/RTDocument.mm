#import "RTDocument.h"
#import <PDFKit/PDFKit.h>
#include "RTCore.h"

NSString *const RTDefaultsFastPreamble = @"RTFastPreamble";
NSString *const RTDefaultsSinglePass = @"RTSinglePass";
NSString *const RTDefaultsTypesetOnSave = @"RTTypesetOnSave";
NSString *const RTDefaultsShellEscape = @"RTShellEscape";
NSString *const RTDefaultsAutoSave = @"RTAutoSave";
NSString *const RTDefaultsKeepBackup = @"RTKeepBackup";
NSString *const RTDefaultsWatchFiles = @"RTWatchFiles";
NSString *const RTDefaultsSaveRelated = @"RTSaveRelated";
NSString *const RTDefaultsEncodingInSavePanel = @"RTEncodingInSavePanel";
NSString *const RTDefaultsPrecomposeUnicode = @"RTPrecomposeUnicode";
NSString *const RTDefaultsAfterBuild = @"RTAfterBuild";  // "keep" | "hide" | "delete"

static NSToolbarItemIdentifier const kTypesetItem = @"rt.typeset";
static NSToolbarItemIdentifier const kStopItem = @"rt.stop";
static NSToolbarItemIdentifier const kEngineItem = @"rt.engine";
static NSToolbarItemIdentifier const kTagsItem = @"rt.tags";
static NSToolbarItemIdentifier const kSyncItem = @"rt.sync";
static NSToolbarItemIdentifier const kBuildItem = @"rt.build";

static NSColor *ColorForToken(uint8_t kind) {
    switch (kind) {
        case RTEX_TOKEN_COMMAND: return NSColor.systemBlueColor;
        case RTEX_TOKEN_COMMENT: return NSColor.systemRedColor;
        case RTEX_TOKEN_MATH: return NSColor.systemPurpleColor;
        case RTEX_TOKEN_BRACE: return NSColor.systemBrownColor;
        case RTEX_TOKEN_MARKER: return NSColor.systemGreenColor;
        case RTEX_TOKEN_INDEX: return NSColor.systemOrangeColor;
        case RTEX_TOKEN_ENVIRONMENT: return NSColor.systemTealColor;
        default: return NSColor.textColor;
    }
}

static NSFont *EditorFont(void) {
    return [NSFont fontWithName:@"Menlo" size:13] ?: [NSFont monospacedSystemFontOfSize:13 weight:NSFontWeightRegular];
}

static NSFont *ConsoleFont(void) {
    return [NSFont monospacedSystemFontOfSize:11 weight:NSFontWeightRegular];
}

// `% !TEX encoding = ...` names (TeXShop spellings) → NSStringEncoding.
static NSStringEncoding EncodingNamed(NSString *name) {
    NSString *n = name.lowercaseString;
    if ([n containsString:@"utf-8"] || [n containsString:@"utf8"]) return NSUTF8StringEncoding;
    if ([n containsString:@"latin2"] || [n containsString:@"latin 2"]) return NSISOLatin2StringEncoding;
    if ([n containsString:@"latin"]) return NSISOLatin1StringEncoding;
    if ([n containsString:@"macosroman"] || [n containsString:@"mac os roman"]) return NSMacOSRomanStringEncoding;
    if ([n containsString:@"windows"] || [n containsString:@"1252"]) return NSWindowsCP1252StringEncoding;
    return 0;
}

static void ConsoleOutput(void *ctx, const char *chunk);

@implementation RTDocument {
    NSString *_loadedText;
    NSStringEncoding _encoding;
    RTTextView *_textView;
    RTLineNumberRuler *_ruler;
    NSTextView *_console;
    RTPDFView *_pdfView;
    NSSplitView *_mainSplit;
    NSSplitView *_leftSplit;
    NSPopUpButton *_enginePopup;
    NSPopUpButton *_tagsPopup;
    NSPopover *_buildPopover;
    NSTextField *_buildFilesLabel;
    rtex_cancel *_cancel;
    BOOL _building;
    BOOL _savingForTypeset;
    NSString *_rootPath;
    NSString *_pdfPath;
    NSDate *_pdfDate;
    NSArray<NSDictionary *> *_issues;  // errors first
    NSUInteger _issueCursor;
    dispatch_source_t _watcher;  // TeXShop: watchFile: / dispatch_source
}

// TeXShop: `autosavesInPlace` returns the AutoSaveEnabled preference (on by
// default), read once at launch.
+ (BOOL)autosavesInPlace {
    static BOOL autosave;
    static dispatch_once_t once;
    dispatch_once(&once, ^{ autosave = [NSUserDefaults.standardUserDefaults boolForKey:RTDefaultsAutoSave]; });
    return autosave;
}

// TeXShop: KeepBackup preference.
- (BOOL)keepBackupFile {
    return [NSUserDefaults.standardUserDefaults boolForKey:RTDefaultsKeepBackup];
}

// TeXShop: a saved file keeps its own extension (.sty stays .sty).
- (NSString *)fileNameExtensionForType:(NSString *)typeName saveOperation:(NSSaveOperationType)op {
    NSString *ext = self.fileURL.pathExtension;
    return ext.length ? ext : @"tex";
}

- (instancetype)init {
    if ((self = [super init])) {
        _encoding = NSUTF8StringEncoding;
        _loadedText = @"";
    }
    return self;
}

// MARK: Reading and writing

- (BOOL)readFromData:(NSData *)data ofType:(NSString *)typeName error:(NSError **)outError {
    NSStringEncoding enc = NSUTF8StringEncoding;
    NSString *text = [[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding];
    // Honour `% !TEX encoding` (look at the head of the file as Latin-1 first).
    NSString *head = [[NSString alloc] initWithData:[data subdataWithRange:NSMakeRange(0, MIN(data.length, (NSUInteger)4096))]
                                           encoding:NSISOLatin1StringEncoding];
    NSDictionary *d = rt::dict(rtex_directives(rt::c(head)));
    if ([d[@"encoding"] isKindOfClass:NSString.class]) {
        NSStringEncoding named = EncodingNamed(d[@"encoding"]);
        if (named && named != NSUTF8StringEncoding) {
            NSString *t = [[NSString alloc] initWithData:data encoding:named];
            if (t) { text = t; enc = named; }
        }
    }
    if (!text) {
        text = [[NSString alloc] initWithData:data encoding:NSISOLatin1StringEncoding];
        enc = NSISOLatin1StringEncoding;
    }
    if (!text) {
        if (outError) *outError = [NSError errorWithDomain:NSCocoaErrorDomain code:NSFileReadInapplicableStringEncodingError userInfo:nil];
        return NO;
    }
    _encoding = enc;
    _loadedText = text;
    if (_textView) {  // revert / reload after an outside change: keep the view where it was
        NSRange sel = _textView.selectedRange;
        NSRect visible = _textView.enclosingScrollView.contentView.bounds;
        [_textView setString:text];
        [self highlightAll];
        [_textView setSelectedRange:NSMakeRange(MIN(sel.location, text.length), 0)];
        [_textView scrollPoint:visible.origin];
    }
    return YES;
}

static NSString *EncodingName(NSStringEncoding e) {
    return [NSString localizedNameOfStringEncoding:e];
}

// TeXShop (`-[TSDocument dataOfType:error:]`): the `% !TEX encoding` line is
// re-read at every save and decides the encoding; holding Option ignores it.
// TeXShop then converts lossily. RTeX never drops characters: without an
// encoding line it switches to UTF-8, with one it refuses and says why.
- (NSData *)dataOfType:(NSString *)typeName error:(NSError **)outError {
    NSString *s = _textView ? _textView.string : _loadedText;
    BOOL declared = NO;
    if (!(NSEvent.modifierFlags & NSEventModifierFlagOption)) {
        NSRange head = [s rangeOfComposedCharacterSequencesForRange:NSMakeRange(0, MIN(s.length, (NSUInteger)4096))];
        NSDictionary *d = rt::dict(rtex_directives(rt::c([s substringWithRange:head])));
        NSStringEncoding named = [d[@"encoding"] isKindOfClass:NSString.class] ? EncodingNamed(d[@"encoding"]) : 0;
        if (named) {
            _encoding = named;
            declared = YES;
        }
    }
    if ([NSUserDefaults.standardUserDefaults boolForKey:RTDefaultsPrecomposeUnicode]) {
        s = s.precomposedStringWithCanonicalMapping;  // TeXShop: AutomaticUTF8MACtoUTF8Conversion
    }
    NSData *data = [s dataUsingEncoding:_encoding allowLossyConversion:NO];
    if (!data && !declared) {
        _encoding = NSUTF8StringEncoding;
        data = [s dataUsingEncoding:NSUTF8StringEncoding];
    }
    if (!data && outError) {
        NSString *msg = [NSString stringWithFormat:
            @"The document has characters that can't be saved as %@, the encoding set by its %% !TEX encoding line.",
            EncodingName(_encoding)];
        *outError = [NSError errorWithDomain:NSCocoaErrorDomain
                                        code:NSFileWriteInapplicableStringEncodingError
                                    userInfo:@{
                                        NSLocalizedDescriptionKey : msg,
                                        NSLocalizedRecoverySuggestionErrorKey :
                                            @"Change the line to % !TEX encoding = UTF-8 Unicode, remove those characters, "
                                            @"or hold Option while saving to ignore the line."
                                    }];
    }
    return data;
}

// TeXShop: EncodingMenuInSaveDialog adds an encoding menu to the save panel.
- (BOOL)prepareSavePanel:(NSSavePanel *)panel {
    if (![NSUserDefaults.standardUserDefaults boolForKey:RTDefaultsEncodingInSavePanel]) return YES;
    NSView *box = [[NSView alloc] initWithFrame:NSMakeRect(0, 0, 320, 40)];
    NSTextField *label = [NSTextField labelWithString:@"Encoding:"];
    label.frame = NSMakeRect(20, 11, 70, 18);
    NSPopUpButton *pop = [[NSPopUpButton alloc] initWithFrame:NSMakeRect(92, 6, 210, 26) pullsDown:NO];
    for (NSNumber *e in @[ @(NSUTF8StringEncoding), @(NSISOLatin1StringEncoding), @(NSISOLatin2StringEncoding),
                           @(NSMacOSRomanStringEncoding), @(NSWindowsCP1252StringEncoding) ]) {
        [pop addItemWithTitle:EncodingName(e.unsignedIntegerValue)];
        pop.lastItem.tag = (NSInteger)e.unsignedIntegerValue;
    }
    [pop selectItemWithTag:(NSInteger)_encoding];
    pop.target = self;
    pop.action = @selector(chooseSaveEncoding:);
    [box addSubview:label];
    [box addSubview:pop];
    panel.accessoryView = box;
    return YES;
}

- (void)chooseSaveEncoding:(NSPopUpButton *)sender {
    _encoding = (NSStringEncoding)sender.selectedTag;
}

// TeXShop: writeToURL: stops the file watcher around its own write and
// breaks undo coalescing afterwards.
- (BOOL)writeToURL:(NSURL *)url ofType:(NSString *)typeName error:(NSError **)outError {
    BOOL ok = [super writeToURL:url ofType:typeName error:outError];
    if (ok) [_textView breakUndoCoalescing];
    return ok;
}

- (void)saveToURL:(NSURL *)url
              ofType:(NSString *)typeName
    forSaveOperation:(NSSaveOperationType)op
   completionHandler:(void (^)(NSError *))completionHandler {
    [self stopWatching];  // our own write must not look like an outside change
    [super saveToURL:url ofType:typeName forSaveOperation:op completionHandler:^(NSError *error) {
        [self startWatching];
        completionHandler(error);
        BOOL userSave = op == NSSaveOperation || op == NSSaveAsOperation;
        if (!error && userSave && !self->_savingForTypeset &&
            [NSUserDefaults.standardUserDefaults boolForKey:RTDefaultsTypesetOnSave]) {
            [self startBuild];
        }
    }];
}

// MARK: Window

- (void)makeWindowControllers {
    NSWindow *w = [[NSWindow alloc] initWithContentRect:NSMakeRect(0, 0, 1320, 820)
                                              styleMask:NSWindowStyleMaskTitled | NSWindowStyleMaskClosable |
                                                        NSWindowStyleMaskMiniaturizable | NSWindowStyleMaskResizable
                                                backing:NSBackingStoreBuffered
                                                  defer:NO];
    w.minSize = NSMakeSize(640, 400);
    w.delegate = self;
    NSWindowController *wc = [[NSWindowController alloc] initWithWindow:w];
    wc.shouldCascadeWindows = YES;
    [self addWindowController:wc];
    [self buildInterfaceInWindow:w];
    [w center];
}

- (void)buildInterfaceInWindow:(NSWindow *)w {
    // Editor (explicit TextKit 1 stack: the line-number ruler uses NSLayoutManager).
    NSScrollView *editorScroll = [[NSScrollView alloc] initWithFrame:NSMakeRect(0, 0, 660, 600)];
    editorScroll.hasVerticalScroller = YES;
    editorScroll.borderType = NSNoBorder;
    NSTextStorage *storage = [[NSTextStorage alloc] init];
    NSLayoutManager *lm = [[NSLayoutManager alloc] init];
    [storage addLayoutManager:lm];
    NSTextContainer *tc = [[NSTextContainer alloc] initWithSize:NSMakeSize(editorScroll.contentSize.width, CGFLOAT_MAX)];
    tc.widthTracksTextView = YES;
    [lm addTextContainer:tc];
    _textView = [[RTTextView alloc] initWithFrame:NSMakeRect(0, 0, editorScroll.contentSize.width, 600) textContainer:tc];
    _textView.minSize = NSMakeSize(0, 0);
    _textView.maxSize = NSMakeSize(CGFLOAT_MAX, CGFLOAT_MAX);
    _textView.verticallyResizable = YES;
    _textView.horizontallyResizable = NO;
    _textView.autoresizingMask = NSViewWidthSizable;
    _textView.richText = NO;
    _textView.importsGraphics = NO;
    _textView.allowsUndo = YES;
    _textView.usesFindBar = YES;
    _textView.incrementalSearchingEnabled = YES;
    _textView.automaticQuoteSubstitutionEnabled = NO;
    _textView.automaticDashSubstitutionEnabled = NO;
    _textView.automaticTextReplacementEnabled = NO;
    _textView.automaticSpellingCorrectionEnabled = NO;
    _textView.continuousSpellCheckingEnabled = NO;
    _textView.smartInsertDeleteEnabled = NO;
    _textView.textContainerInset = NSMakeSize(4, 6);
    _textView.delegate = self;
    _textView.syncDelegate = self;
    NSFont *font = EditorFont();
    NSMutableParagraphStyle *para = [[NSMutableParagraphStyle alloc] init];
    para.tabStops = @[];
    para.defaultTabInterval = 4 * [@" " sizeWithAttributes:@{NSFontAttributeName : font}].width;
    _textView.defaultParagraphStyle = para;
    _textView.font = font;
    _textView.typingAttributes = @{NSFontAttributeName : font, NSParagraphStyleAttributeName : para,
                                   NSForegroundColorAttributeName : NSColor.textColor};
    storage.delegate = self;
    editorScroll.documentView = _textView;
    _ruler = [[RTLineNumberRuler alloc] initWithTextView:_textView];
    editorScroll.verticalRulerView = _ruler;
    editorScroll.hasVerticalRuler = YES;
    editorScroll.rulersVisible = YES;

    [_textView setString:_loadedText];
    [storage addAttributes:@{NSFontAttributeName : font, NSParagraphStyleAttributeName : para}
                     range:NSMakeRange(0, storage.length)];
    [self highlightAll];
    [_textView setSelectedRange:NSMakeRange(0, 0)];

    // Console (TeXShop shows this in a separate window; here it sits below the editor).
    NSScrollView *consoleScroll = [[NSScrollView alloc] initWithFrame:NSMakeRect(0, 0, 660, 180)];
    consoleScroll.hasVerticalScroller = YES;
    consoleScroll.borderType = NSNoBorder;
    _console = [[NSTextView alloc] initWithFrame:NSMakeRect(0, 0, consoleScroll.contentSize.width, 180)];
    _console.editable = NO;
    _console.richText = YES;
    _console.verticallyResizable = YES;
    _console.autoresizingMask = NSViewWidthSizable;
    _console.textContainer.widthTracksTextView = YES;
    _console.font = ConsoleFont();
    _console.textContainerInset = NSMakeSize(4, 4);
    _console.backgroundColor = NSColor.underPageBackgroundColor;
    _console.delegate = self;
    consoleScroll.documentView = _console;

    _leftSplit = [[NSSplitView alloc] initWithFrame:NSMakeRect(0, 0, 660, 800)];
    _leftSplit.vertical = NO;
    _leftSplit.dividerStyle = NSSplitViewDividerStyleThin;
    [_leftSplit addArrangedSubview:editorScroll];
    [_leftSplit addArrangedSubview:consoleScroll];
    [_leftSplit setHoldingPriority:NSLayoutPriorityDefaultLow - 1 forSubviewAtIndex:0];
    [_leftSplit setHoldingPriority:NSLayoutPriorityDefaultLow + 1 forSubviewAtIndex:1];

    _pdfView = [[RTPDFView alloc] initWithFrame:NSMakeRect(0, 0, 660, 800)];
    _pdfView.autoScales = YES;
    _pdfView.displayMode = kPDFDisplaySinglePageContinuous;
    _pdfView.displaysPageBreaks = YES;
    _pdfView.syncDelegate = self;

    _mainSplit = [[NSSplitView alloc] initWithFrame:w.contentView.bounds];
    _mainSplit.vertical = YES;
    _mainSplit.dividerStyle = NSSplitViewDividerStyleThin;
    _mainSplit.autoresizingMask = NSViewWidthSizable | NSViewHeightSizable;
    [_mainSplit addArrangedSubview:_leftSplit];
    [_mainSplit addArrangedSubview:_pdfView];
    w.contentView = _mainSplit;
    [w layoutIfNeeded];
    [_mainSplit setPosition:NSWidth(_mainSplit.bounds) * 0.5 ofDividerAtIndex:0];
    [_leftSplit setPosition:NSHeight(_leftSplit.bounds) * 0.76 ofDividerAtIndex:0];

    NSToolbar *toolbar = [[NSToolbar alloc] initWithIdentifier:@"RTDocumentToolbar2"];
    toolbar.delegate = self;
    toolbar.displayMode = NSToolbarDisplayModeIconAndLabel;
    toolbar.allowsUserCustomization = YES;
    toolbar.autosavesConfiguration = YES;
    w.toolbar = toolbar;
    w.toolbarStyle = NSWindowToolbarStyleExpanded;
    w.initialFirstResponder = _textView;

    [self appendConsole:@"Ready. ⌘T typesets, ⌘-click in the source or the PDF to sync.\n"
                  color:NSColor.secondaryLabelColor];
    [self locatePDF];
    [self startWatching];
}

// MARK: Watching the file for outside changes (TeXShop: watchFile:)

- (void)startWatching {
    [self stopWatching];
    if (![NSUserDefaults.standardUserDefaults boolForKey:RTDefaultsWatchFiles] || !self.fileURL) return;
    int fd = open(self.fileURL.fileSystemRepresentation, O_EVTONLY);
    if (fd < 0) return;
    dispatch_source_t src = dispatch_source_create(DISPATCH_SOURCE_TYPE_VNODE, (uintptr_t)fd,
                                                   DISPATCH_VNODE_DELETE | DISPATCH_VNODE_WRITE | DISPATCH_VNODE_RENAME,
                                                   dispatch_get_main_queue());
    __weak RTDocument *weakSelf = self;
    __weak dispatch_source_t weakSrc = src;
    dispatch_source_set_event_handler(src, ^{
        dispatch_source_t strongSrc = weakSrc;
        if (strongSrc) [weakSelf fileChangedOnDisk:dispatch_source_get_data(strongSrc)];
    });
    dispatch_source_set_cancel_handler(src, ^{ close(fd); });
    _watcher = src;
    dispatch_resume(src);
}

- (void)stopWatching {
    if (_watcher) {
        dispatch_source_cancel(_watcher);
        _watcher = nil;
    }
}

// TeXShop's handler: with unsaved edits, stop watching (never overwrite the
// user's work); a write reloads; a delete is re-checked after a second
// because editors that save by replacing the file look like that. TeXShop
// closes the window when the file is really gone or renamed; RTeX keeps it
// open and marks it unsaved instead.
- (void)fileChangedOnDisk:(unsigned long)flags {
    if (self.hasUnautosavedChanges || self.isDocumentEdited) {
        [self stopWatching];
        [self appendConsole:@"\nThis file changed on disk, but you have unsaved edits here; they were kept. "
                            @"File → Revert To shows the version on disk.\n"
                      color:NSColor.systemOrangeColor];
        return;
    }
    if (flags & DISPATCH_VNODE_WRITE) {
        [self reloadFromDisk];
        return;
    }
    [self stopWatching];
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, NSEC_PER_SEC), dispatch_get_main_queue(), ^{
        if (self.fileURL && [NSFileManager.defaultManager isReadableFileAtPath:self.fileURL.path]) {
            [self reloadFromDisk];
        } else {
            [self updateChangeCount:NSChangeDone];
            [self appendConsole:@"\nThis file was moved or deleted on disk. Its text is still here; save to write it again.\n"
                          color:NSColor.systemOrangeColor];
        }
    });
}

- (void)reloadFromDisk {
    NSError *error = nil;
    if ([self revertToContentsOfURL:self.fileURL ofType:self.fileType error:&error]) {
        [self appendConsole:@"\nReloaded: the file was changed by another application.\n" color:NSColor.secondaryLabelColor];
    }
    [self startWatching];
}

- (void)windowDidBecomeKey:(NSNotification *)n {
    // Pick up PDFs rebuilt outside the app (e.g. `rtex watch`).
    [self locatePDF];
}

// MARK: Toolbar

- (NSArray<NSToolbarItemIdentifier> *)toolbarDefaultItemIdentifiers:(NSToolbar *)toolbar {
    return @[ kTypesetItem, kStopItem, kEngineItem, kBuildItem, NSToolbarFlexibleSpaceItemIdentifier, kTagsItem, kSyncItem ];
}

- (NSArray<NSToolbarItemIdentifier> *)toolbarAllowedItemIdentifiers:(NSToolbar *)toolbar {
    return @[ kTypesetItem, kStopItem, kEngineItem, kBuildItem, kTagsItem, kSyncItem, NSToolbarFlexibleSpaceItemIdentifier,
              NSToolbarSpaceItemIdentifier ];
}

- (NSToolbarItem *)toolbar:(NSToolbar *)toolbar
        itemForItemIdentifier:(NSToolbarItemIdentifier)ident
    willBeInsertedIntoToolbar:(BOOL)flag {
    NSToolbarItem *item = [[NSToolbarItem alloc] initWithItemIdentifier:ident];
    item.target = self;
    if ([ident isEqual:kTypesetItem]) {
        item.label = @"Typeset";
        item.toolTip = @"Typeset (⌘T)";
        item.image = [NSImage imageWithSystemSymbolName:@"play.fill" accessibilityDescription:@"Typeset"];
        item.action = @selector(typeset:);
    } else if ([ident isEqual:kStopItem]) {
        item.label = @"Stop";
        item.toolTip = @"Stop typesetting (⌘.)";
        item.image = [NSImage imageWithSystemSymbolName:@"stop.fill" accessibilityDescription:@"Stop"];
        item.action = @selector(stopTypesetting:);
    } else if ([ident isEqual:kSyncItem]) {
        item.label = @"Sync";
        item.toolTip = @"Show the cursor line in the PDF (⌘J)";
        item.image = [NSImage imageWithSystemSymbolName:@"arrow.left.and.right.square" accessibilityDescription:@"Sync"];
        item.action = @selector(syncToPDF:);
    } else if ([ident isEqual:kEngineItem]) {
        item.label = @"Engine";
        _enginePopup = [[NSPopUpButton alloc] initWithFrame:NSMakeRect(0, 0, 150, 26) pullsDown:NO];
        [self fillEngines];
        item.view = _enginePopup;
    } else if ([ident isEqual:kBuildItem]) {
        item.label = @"Build";
        item.toolTip = @"Typesetting options and build-file clean-up";
        item.image = [NSImage imageWithSystemSymbolName:@"slider.horizontal.3" accessibilityDescription:@"Build options"];
        item.action = @selector(showBuildPanel:);
    } else if ([ident isEqual:kTagsItem]) {
        item.label = @"Tags";
        _tagsPopup = [[NSPopUpButton alloc] initWithFrame:NSMakeRect(0, 0, 180, 26) pullsDown:YES];
        _tagsPopup.menu.delegate = self;
        [_tagsPopup addItemWithTitle:@"Tags"];
        item.view = _tagsPopup;
    }
    return item;
}

- (void)fillEngines {
    [_enginePopup removeAllItems];
    [_enginePopup addItemWithTitle:@"Default"];
    _enginePopup.lastItem.toolTip = @"% !TEX program, else pdflatex";
    id engines = rt::json(rtex_engines());
    if (![engines isKindOfClass:NSArray.class]) return;
    BOOL separated = NO;
    for (NSDictionary *e in engines) {
        if (![e[@"kind"] isEqual:@"builtin"] && !separated) {
            [_enginePopup.menu addItem:NSMenuItem.separatorItem];
            separated = YES;
        }
        [_enginePopup addItemWithTitle:e[@"name"]];
        _enginePopup.lastItem.representedObject = e[@"name"];
    }
}

- (BOOL)validateUserInterfaceItem:(id<NSValidatedUserInterfaceItem>)item {
    SEL a = item.action;
    if (a == @selector(typeset:)) return !_building;
    if (a == @selector(stopTypesetting:)) return _building;
    if (a == @selector(syncToPDF:)) return _pdfPath != nil;
    if (a == @selector(goToNextError:)) return _issues.count > 0;
    return [super validateUserInterfaceItem:item];
}

// MARK: Tags menu

- (void)menuNeedsUpdate:(NSMenu *)menu {
    if (menu != _tagsPopup.menu) return;
    while (menu.numberOfItems > 1) [menu removeItemAtIndex:1];
    id entries = rt::json(rtex_outline(rt::c(_textView.string)));
    if (![entries isKindOfClass:NSArray.class]) return;
    NSMenu *labels = [[NSMenu alloc] initWithTitle:@"Labels"];
    for (NSDictionary *e in entries) {
        NSString *kind = e[@"kind"];
        NSInteger level = [e[@"level"] integerValue];
        NSString *title = e[@"title"];
        NSMenuItem *mi = [[NSMenuItem alloc] initWithTitle:@"" action:@selector(jumpToTag:) keyEquivalent:@""];
        mi.target = self;
        mi.representedObject = e[@"offset16"];
        if ([kind isEqual:@"label"]) {
            mi.title = title;
            [labels addItem:mi];
            continue;
        }
        if ([kind isEqual:@"mark"]) {
            mi.title = [@"%: " stringByAppendingString:title];
        } else {
            NSString *indent = [@"" stringByPaddingToLength:(NSUInteger)MIN(level, 5) * 3 withString:@" " startingAtIndex:0];
            mi.title = [indent stringByAppendingString:title];
        }
        [menu addItem:mi];
    }
    if (labels.numberOfItems) {
        [menu addItem:NSMenuItem.separatorItem];
        NSMenuItem *sub = [[NSMenuItem alloc] initWithTitle:@"Labels" action:nil keyEquivalent:@""];
        sub.submenu = labels;
        [menu addItem:sub];
    }
    if (menu.numberOfItems == 1) [menu addItemWithTitle:@"(no sections)" action:nil keyEquivalent:@""];
}

- (void)jumpToTag:(NSMenuItem *)sender {
    NSUInteger off = [sender.representedObject unsignedIntegerValue];
    [self selectLine:[_textView lineForIndex:off]];
}

// MARK: Highlighting

- (void)highlightAll {
    [self highlightRange:NSMakeRange(0, _textView.string.length)];
}

- (void)highlightRange:(NSRange)range {
    NSString *s = _textView.string;
    if (range.location > s.length) return;
    range.length = MIN(range.length, s.length - range.location);
    range = [s lineRangeForRange:range];
    NSLayoutManager *lm = _textView.layoutManager;
    [lm removeTemporaryAttribute:NSForegroundColorAttributeName forCharacterRange:range];
    if (!range.length) return;
    for (const rt::Span &sp : rt::highlight([s substringWithRange:range])) {
        NSRange r = NSMakeRange(range.location + sp.range.location, sp.range.length);
        if (NSMaxRange(r) > NSMaxRange(range)) continue;
        [lm addTemporaryAttribute:NSForegroundColorAttributeName value:ColorForToken(sp.kind) forCharacterRange:r];
    }
}

- (void)textStorage:(NSTextStorage *)ts
    didProcessEditing:(NSTextStorageEditActions)actions
                range:(NSRange)edited
       changeInLength:(NSInteger)delta {
    if (!(actions & NSTextStorageEditedCharacters)) return;
    // Temporary attributes live on the layout manager; apply them once the
    // storage has finished processing this edit.
    dispatch_async(dispatch_get_main_queue(), ^{ [self highlightRange:edited]; });
}

// MARK: Typesetting

- (IBAction)typeset:(id)sender {
    if (_building) return;
    [self saveRelatedDocumentsThen:^{ [self saveSelfAndBuild]; }];
}

- (void)saveSelfAndBuild {
    if (!self.fileURL || self.isDocumentEdited) {
        _savingForTypeset = YES;
        [self saveDocumentWithDelegate:self didSaveSelector:@selector(document:didSaveForTypeset:contextInfo:) contextInfo:NULL];
    } else {
        [self startBuild];
    }
}

// TeXShop SaveRelated (`checkFileLinks:`): before typesetting, save the other
// open documents of this project that have unsaved edits: the root and the
// files it \input/\include/\subfile's.
- (void)saveRelatedDocumentsThen:(void (^)(void))next {
    if (![NSUserDefaults.standardUserDefaults boolForKey:RTDefaultsSaveRelated] || !self.fileURL) {
        next();
        return;
    }
    NSDictionary *r = rt::dict(rtex_resolve_root(self.fileURL.path.UTF8String));
    NSString *root = [r[@"root"] isKindOfClass:NSString.class] ? r[@"root"] : self.fileURL.path;
    NSArray<NSDocument *> *docs = NSDocumentController.sharedDocumentController.documents;
    NSString *rootText = nil;
    for (NSDocument *d in docs) {
        if ([d isKindOfClass:RTDocument.class] && [d.fileURL.path.stringByStandardizingPath isEqual:root.stringByStandardizingPath]) {
            rootText = [(RTDocument *)d currentText];
        }
    }
    if (!rootText) rootText = [NSString stringWithContentsOfFile:root usedEncoding:NULL error:nil] ?: @"";
    NSMutableSet<NSString *> *project = [NSMutableSet setWithObject:root.stringByStandardizingPath];
    id files = rt::json(rtex_included_files(rootText.UTF8String, root.stringByDeletingLastPathComponent.UTF8String));
    if ([files isKindOfClass:NSArray.class]) {
        for (NSString *f in files) [project addObject:f.stringByStandardizingPath];
    }
    dispatch_group_t group = dispatch_group_create();
    for (NSDocument *d in docs) {
        if (d == self || ![d isKindOfClass:RTDocument.class] || !d.fileURL || !d.isDocumentEdited) continue;
        if (![project containsObject:d.fileURL.path.stringByStandardizingPath]) continue;
        dispatch_group_enter(group);
        [(RTDocument *)d saveWithoutTypesetting:^{ dispatch_group_leave(group); }];
    }
    dispatch_group_notify(group, dispatch_get_main_queue(), next);
}

- (NSString *)currentText {
    return _textView ? _textView.string : _loadedText;
}

- (void)saveWithoutTypesetting:(void (^)(void))done {
    _savingForTypeset = YES;
    [self saveToURL:self.fileURL ofType:self.fileType forSaveOperation:NSSaveOperation completionHandler:^(NSError *error) {
        self->_savingForTypeset = NO;
        done();
    }];
}

- (void)document:(NSDocument *)doc didSaveForTypeset:(BOOL)didSave contextInfo:(void *)ctx {
    _savingForTypeset = NO;
    if (didSave) [self startBuild];
}

- (NSString *)buildOptionsJSON {
    NSUserDefaults *ud = NSUserDefaults.standardUserDefaults;
    NSMutableDictionary *o = [@{
        @"fast_preamble" : @([ud boolForKey:RTDefaultsFastPreamble]),
        @"single_pass" : @([ud boolForKey:RTDefaultsSinglePass]),
        @"shell_escape" : @([ud boolForKey:RTDefaultsShellEscape]),
        @"after_build" : [ud stringForKey:RTDefaultsAfterBuild] ?: @"keep",
    } mutableCopy];
    NSString *engine = _enginePopup.selectedItem.representedObject;
    if (engine) o[@"engine"] = engine;
    NSData *d = [NSJSONSerialization dataWithJSONObject:o options:0 error:nil];
    return [[NSString alloc] initWithData:d encoding:NSUTF8StringEncoding];
}

- (void)startBuild {
    if (_building || !self.fileURL) return;
    _building = YES;
    _issues = nil;
    [self.windowControllers.firstObject.window.toolbar validateVisibleItems];
    self.windowControllers.firstObject.window.subtitle = @"Typesetting…";
    [_console.textStorage setAttributedString:[[NSAttributedString alloc] init]];

    NSString *path = self.fileURL.path;
    NSString *opts = [self buildOptionsJSON];
    rtex_cancel *cancel = rtex_cancel_new();
    _cancel = cancel;
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
        NSString *error = nil;
        NSDictionary *report = rt::dict(rtex_build(path.UTF8String, opts.UTF8String, ConsoleOutput,
                                                   (__bridge void *)self, cancel),
                                        &error);
        dispatch_async(dispatch_get_main_queue(), ^{
            self->_cancel = nullptr;
            rtex_cancel_free(cancel);
            [self finishBuild:report error:error];
        });
    });
}

static void ConsoleOutput(void *ctx, const char *chunk) {
    NSString *s = [NSString stringWithUTF8String:chunk];
    if (!s.length) return;
    RTDocument *doc = (__bridge RTDocument *)ctx;
    dispatch_async(dispatch_get_main_queue(), ^{ [doc appendConsole:s color:nil]; });
}

- (IBAction)stopTypesetting:(id)sender {
    if (_cancel) rtex_cancel_set(_cancel);
}

- (void)finishBuild:(NSDictionary *)report error:(NSString *)error {
    _building = NO;
    [self refreshBuildFilesLabel];
    NSWindow *w = self.windowControllers.firstObject.window;
    [w.toolbar validateVisibleItems];
    if (!report) {
        [self appendConsole:[NSString stringWithFormat:@"\nrtex error: %@\n", error] color:NSColor.systemRedColor];
        w.subtitle = @"Failed";
        return;
    }
    id root = report[@"root"], pdf = report[@"pdf"];
    if ([root isKindOfClass:NSString.class]) _rootPath = root;
    if ([pdf isKindOfClass:NSString.class]) {
        _pdfPath = pdf;
        [self reloadPDF];
    }
    if ([report[@"error"] isKindOfClass:NSString.class]) {
        [self appendConsole:[NSString stringWithFormat:@"\n%@\n", report[@"error"]] color:NSColor.systemRedColor];
    }

    // Summary with clickable issues.
    NSArray *messages = report[@"log"][@"messages"];
    NSMutableArray *errors = [NSMutableArray array], *warnings = [NSMutableArray array];
    NSUInteger badboxes = 0;
    if ([messages isKindOfClass:NSArray.class]) {
        for (NSDictionary *m in messages) {
            if ([m[@"level"] isEqual:@"error"]) [errors addObject:m];
            else if ([m[@"level"] isEqual:@"warning"]) [warnings addObject:m];
            else badboxes++;
        }
    }
    _issues = [errors arrayByAddingObjectsFromArray:warnings];
    _issueCursor = 0;
    [self appendConsole:@"\n────────────────────────────────────────\n" color:NSColor.separatorColor];
    for (NSDictionary *m in _issues) [self appendIssue:m];

    NSInteger passes = [report[@"passes"] integerValue];
    double secs = [report[@"total_millis"] doubleValue] / 1000.0;
    NSString *status;
    if ([report[@"cancelled"] boolValue]) {
        status = @"Stopped";
    } else if (errors.count) {
        status = [NSString stringWithFormat:@"%lu error%@", (unsigned long)errors.count, errors.count == 1 ? @"" : @"s"];
        NSBeep();
    } else if ([report[@"success"] boolValue]) {
        id pages = report[@"pages"];
        status = [NSString stringWithFormat:@"%ld pass%@ · %.2f s%@", (long)passes, passes == 1 ? @"" : @"es", secs,
                                            [pages isKindOfClass:NSNumber.class] ? [NSString stringWithFormat:@" · %@ pages", pages] : @""];
    } else {
        status = @"Failed";
    }
    if (warnings.count) status = [status stringByAppendingFormat:@" · %lu warning%@", (unsigned long)warnings.count, warnings.count == 1 ? @"" : @"s"];
    if (badboxes) status = [status stringByAppendingFormat:@" · %lu bad box%@", (unsigned long)badboxes, badboxes == 1 ? @"" : @"es"];
    w.subtitle = status;
    [self appendConsole:[status stringByAppendingString:@"\n"]
                  color:errors.count ? NSColor.systemRedColor : NSColor.secondaryLabelColor];
}

- (NSString *)resolveLogPath:(NSString *)file {
    if (!file.length) return _rootPath ?: self.fileURL.path;
    if (file.isAbsolutePath) return file.stringByStandardizingPath;
    NSString *dir = (_rootPath ?: self.fileURL.path).stringByDeletingLastPathComponent;
    return [dir stringByAppendingPathComponent:file].stringByStandardizingPath;
}

- (void)appendIssue:(NSDictionary *)m {
    BOOL isError = [m[@"level"] isEqual:@"error"];
    NSString *file = [m[@"file"] isKindOfClass:NSString.class] ? m[@"file"] : nil;
    NSNumber *line = [m[@"line"] isKindOfClass:NSNumber.class] ? m[@"line"] : nil;
    NSString *where = [NSString stringWithFormat:@"%@%@", file ? file.lastPathComponent : @"",
                                                 line ? [NSString stringWithFormat:@":%@", line] : @""];
    NSMutableAttributedString *s = [[NSMutableAttributedString alloc] init];
    NSDictionary *base = @{NSFontAttributeName : ConsoleFont(),
                           NSForegroundColorAttributeName : isError ? NSColor.systemRedColor : NSColor.systemOrangeColor};
    [s appendAttributedString:[[NSAttributedString alloc] initWithString:isError ? @"✖ " : @"⚠︎ " attributes:base]];
    if (where.length && line) {
        NSURLComponents *u = [NSURLComponents componentsWithString:@"rtex-goto://issue"];
        u.queryItems = @[ [NSURLQueryItem queryItemWithName:@"file" value:[self resolveLogPath:file]],
                          [NSURLQueryItem queryItemWithName:@"line" value:line.stringValue] ];
        NSMutableDictionary *link = [base mutableCopy];
        link[NSLinkAttributeName] = u.URL;
        [s appendAttributedString:[[NSAttributedString alloc] initWithString:where attributes:link]];
        [s appendAttributedString:[[NSAttributedString alloc] initWithString:@"  " attributes:base]];
    }
    NSDictionary *text = @{NSFontAttributeName : ConsoleFont(), NSForegroundColorAttributeName : NSColor.labelColor};
    [s appendAttributedString:[[NSAttributedString alloc] initWithString:m[@"message"] attributes:text]];
    if ([m[@"context"] isKindOfClass:NSString.class]) {
        NSString *ctx = [NSString stringWithFormat:@"\n    %@", m[@"context"]];
        [s appendAttributedString:[[NSAttributedString alloc]
                                      initWithString:ctx
                                          attributes:@{NSFontAttributeName : ConsoleFont(),
                                                       NSForegroundColorAttributeName : NSColor.secondaryLabelColor}]];
    }
    [s appendAttributedString:[[NSAttributedString alloc] initWithString:@"\n" attributes:text]];
    [_console.textStorage appendAttributedString:s];
    [_console scrollRangeToVisible:NSMakeRange(_console.string.length, 0)];
}

- (void)appendConsole:(NSString *)text color:(NSColor *)color {
    NSDictionary *attrs = @{NSFontAttributeName : ConsoleFont(),
                            NSForegroundColorAttributeName : color ?: NSColor.labelColor};
    [_console.textStorage appendAttributedString:[[NSAttributedString alloc] initWithString:text attributes:attrs]];
    [_console scrollRangeToVisible:NSMakeRange(_console.string.length, 0)];
}

- (BOOL)textView:(NSTextView *)tv clickedOnLink:(id)link atIndex:(NSUInteger)charIndex {
    if (tv != _console) return NO;
    NSURL *url = [link isKindOfClass:NSURL.class] ? link : [NSURL URLWithString:link];
    if (![url.scheme isEqual:@"rtex-goto"]) return NO;
    NSString *file = nil;
    NSUInteger line = 0;
    for (NSURLQueryItem *q in [NSURLComponents componentsWithURL:url resolvingAgainstBaseURL:NO].queryItems) {
        if ([q.name isEqual:@"file"]) file = q.value;
        if ([q.name isEqual:@"line"]) line = (NSUInteger)q.value.integerValue;
    }
    [self openFile:file atLine:line];
    return YES;
}

- (IBAction)goToNextError:(id)sender {
    if (!_issues.count) return;
    NSDictionary *m = _issues[_issueCursor % _issues.count];
    _issueCursor++;
    NSNumber *line = [m[@"line"] isKindOfClass:NSNumber.class] ? m[@"line"] : nil;
    NSString *file = [m[@"file"] isKindOfClass:NSString.class] ? m[@"file"] : nil;
    if (line) [self openFile:[self resolveLogPath:file] atLine:line.unsignedIntegerValue];
}

// MARK: Build panel (typesetting options + build-file clean-up)

- (NSButton *)panelCheckbox:(NSString *)title key:(NSString *)key {
    NSButton *b = [NSButton checkboxWithTitle:title target:self action:@selector(panelToggle:)];
    b.identifier = key;
    b.state = [NSUserDefaults.standardUserDefaults boolForKey:key] ? NSControlStateValueOn : NSControlStateValueOff;
    return b;
}

- (void)panelToggle:(NSButton *)sender {
    [NSUserDefaults.standardUserDefaults setBool:sender.state == NSControlStateValueOn forKey:sender.identifier];
}

- (NSTextField *)panelNote:(NSString *)text {
    NSTextField *t = [NSTextField wrappingLabelWithString:text];
    t.font = [NSFont systemFontOfSize:NSFont.smallSystemFontSize];
    t.textColor = NSColor.secondaryLabelColor;
    t.preferredMaxLayoutWidth = 300;
    return t;
}

- (NSView *)buildPanelView {
    NSString *mode = [NSUserDefaults.standardUserDefaults stringForKey:RTDefaultsAfterBuild] ?: @"keep";
    NSTextField *heading = [NSTextField labelWithString:@"After typesetting"];
    heading.font = [NSFont boldSystemFontOfSize:NSFont.systemFontSize];
    NSMutableArray<NSView *> *rows = [NSMutableArray arrayWithObject:heading];
    NSArray *choices = @[
        @[ @"keep", @"Keep build files", @"Leave .aux, .log, .toc… next to your document." ],
        @[ @"hide", @"Hide build files", @"The folder shows only .tex and .pdf. Build files wait in RTeX's cache, so rebuilds stay fast." ],
        @[ @"delete", @"Delete build files", @"Removed after every typeset; the next typeset starts from scratch (slower)." ],
    ];
    for (NSArray *c in choices) {
        NSButton *r = [NSButton radioButtonWithTitle:c[1] target:self action:@selector(panelAfterBuild:)];
        r.identifier = c[0];
        r.state = [mode isEqual:c[0]] ? NSControlStateValueOn : NSControlStateValueOff;
        NSTextField *note = [self panelNote:c[2]];
        note.preferredMaxLayoutWidth = 300;
        NSStackView *choice = [NSStackView stackViewWithViews:@[ r, note ]];
        choice.orientation = NSUserInterfaceLayoutOrientationVertical;
        choice.alignment = NSLayoutAttributeLeading;
        choice.spacing = 1;
        choice.edgeInsets = NSEdgeInsetsMake(0, 0, 2, 0);
        [note.leadingAnchor constraintEqualToAnchor:choice.leadingAnchor constant:20].active = YES;
        [rows addObject:choice];
    }
    [rows addObject:[self panelNote:@"Only files TeX generates are touched, never your .bib, images or packages. "
                                    @"⌘-click sync works in every mode."]];
    NSBox *sep1 = [[NSBox alloc] init];
    sep1.boxType = NSBoxSeparator;
    [rows addObject:sep1];
    NSTextField *opts = [NSTextField labelWithString:@"Typesetting"];
    opts.font = heading.font;
    [rows addObject:opts];
    [rows addObject:[self panelCheckbox:@"Precompiled preamble (pdflatex, faster)" key:RTDefaultsFastPreamble]];
    [rows addObject:[self panelCheckbox:@"Single pass (no BibTeX, index or reruns)" key:RTDefaultsSinglePass]];
    [rows addObject:[self panelCheckbox:@"Typeset on save" key:RTDefaultsTypesetOnSave]];
    [rows addObject:[self panelCheckbox:@"Save project files before typesetting" key:RTDefaultsSaveRelated]];
    [rows addObject:[self panelCheckbox:@"Allow shell escape" key:RTDefaultsShellEscape]];
    NSBox *sep2 = [[NSBox alloc] init];
    sep2.boxType = NSBoxSeparator;
    [rows addObject:sep2];
    _buildFilesLabel = [self panelNote:@""];
    [rows addObject:_buildFilesLabel];
    NSButton *trash = [NSButton buttonWithTitle:@"Trash Build Files Now" target:self action:@selector(trashAuxFiles:)];
    NSButton *go = [NSButton buttonWithTitle:@"Typeset" target:self action:@selector(typesetFromPanel:)];
    go.keyEquivalent = @"\r";
    NSStackView *buttons = [NSStackView stackViewWithViews:@[ trash, go ]];
    [rows addObject:buttons];

    NSStackView *stack = [NSStackView stackViewWithViews:rows];
    stack.orientation = NSUserInterfaceLayoutOrientationVertical;
    stack.alignment = NSLayoutAttributeLeading;
    stack.spacing = 8;
    stack.edgeInsets = NSEdgeInsetsMake(14, 16, 14, 16);
    for (NSView *v in @[ sep1, sep2 ]) [v.widthAnchor constraintEqualToAnchor:stack.widthAnchor constant:-32].active = YES;
    [stack.widthAnchor constraintEqualToConstant:360].active = YES;
    return stack;
}

- (IBAction)showBuildPanel:(id)sender {
    if (_buildPopover.shown) {
        [_buildPopover close];
        return;
    }
    NSViewController *vc = [[NSViewController alloc] init];
    vc.view = [self buildPanelView];
    [self refreshBuildFilesLabel];
    _buildPopover = [[NSPopover alloc] init];
    _buildPopover.behavior = NSPopoverBehaviorTransient;
    _buildPopover.contentViewController = vc;
    NSWindow *w = self.windowControllers.firstObject.window;
    NSToolbarItem *toolbarItem = nil;
    for (NSToolbarItem *it in w.toolbar.items) {
        if ([it.itemIdentifier isEqual:kBuildItem]) toolbarItem = it;
    }
    if (@available(macOS 14.0, *)) {
        if (toolbarItem && w.toolbar.visible) {
            [_buildPopover showRelativeToToolbarItem:toolbarItem];
            return;
        }
    }
    NSView *content = w.contentView;
    NSRect r = NSMakeRect(NSMidX(content.bounds), NSMaxY(content.bounds) - 2, 1, 1);
    [_buildPopover showRelativeToRect:r ofView:content preferredEdge:NSRectEdgeMaxY];
}

- (void)panelAfterBuild:(NSButton *)sender {
    [NSUserDefaults.standardUserDefaults setObject:sender.identifier forKey:RTDefaultsAfterBuild];
}

- (void)typesetFromPanel:(id)sender {
    [_buildPopover close];
    [self typeset:sender];
}

- (void)refreshBuildFilesLabel {
    if (!_buildFilesLabel) return;
    if (!self.fileURL) {
        _buildFilesLabel.stringValue = @"Save the document to see its build files.";
        return;
    }
    id files = rt::json(rtex_build_files(self.fileURL.path.UTF8String));
    NSArray *names = [files isKindOfClass:NSArray.class] ? [files valueForKey:@"lastPathComponent"] : @[];
    _buildFilesLabel.stringValue = names.count
        ? [NSString stringWithFormat:@"In the folder now: %lu build file%@ (%@)", (unsigned long)names.count,
                                     names.count == 1 ? @"" : @"s", [names componentsJoinedByString:@", "]]
        : @"No build files in the folder.";
}

- (IBAction)trashAuxFiles:(id)sender {
    if (!self.fileURL) return;
    BOOL aggressive = (NSEvent.modifierFlags & NSEventModifierFlagOption) != 0;
    id removed = rt::json(rtex_clean(self.fileURL.path.UTF8String, aggressive));
    NSUInteger n = [removed isKindOfClass:NSArray.class] ? [removed count] : 0;
    [self appendConsole:[NSString stringWithFormat:@"\nRemoved %lu build file%@%@.\n", (unsigned long)n,
                                                   n == 1 ? @"" : @"s", aggressive ? @" (whole folder)" : @""]
                  color:NSColor.secondaryLabelColor];
    [self refreshBuildFilesLabel];
}

// Typeset menu → After Typesetting.
- (IBAction)setAfterBuild:(NSMenuItem *)sender {
    [NSUserDefaults.standardUserDefaults setObject:sender.representedObject forKey:RTDefaultsAfterBuild];
}

- (BOOL)validateMenuItem:(NSMenuItem *)item {
    if (item.action == @selector(setAfterBuild:)) {
        NSString *mode = [NSUserDefaults.standardUserDefaults stringForKey:RTDefaultsAfterBuild] ?: @"keep";
        item.state = [mode isEqual:item.representedObject] ? NSControlStateValueOn : NSControlStateValueOff;
        return YES;
    }
    return [super validateMenuItem:item];
}

// MARK: PDF

// Find the root's PDF without typesetting (on open, and when re-activated).
- (void)locatePDF {
    if (!self.fileURL) return;
    if (!_rootPath) {
        NSDictionary *r = rt::dict(rtex_resolve_root(self.fileURL.path.UTF8String));
        _rootPath = [r[@"root"] isKindOfClass:NSString.class] ? r[@"root"] : self.fileURL.path;
    }
    NSString *pdf = [_rootPath.stringByDeletingPathExtension stringByAppendingPathExtension:@"pdf"];
    if (![NSFileManager.defaultManager fileExistsAtPath:pdf]) return;
    NSDate *date = [NSFileManager.defaultManager attributesOfItemAtPath:pdf error:nil].fileModificationDate;
    if (_pdfPath && [_pdfPath isEqual:pdf] && [date isEqual:_pdfDate]) return;
    _pdfPath = pdf;
    [self reloadPDF];
}

- (void)reloadPDF {
    if (!_pdfPath) return;
    PDFDocument *doc = [[PDFDocument alloc] initWithURL:[NSURL fileURLWithPath:_pdfPath]];
    if (!doc) return;
    _pdfDate = [NSFileManager.defaultManager attributesOfItemAtPath:_pdfPath error:nil].fileModificationDate;
    PDFDocument *old = _pdfView.document;
    PDFDestination *dest = old ? _pdfView.currentDestination : nil;
    NSUInteger index = (dest.page && old) ? [old indexForPage:dest.page] : 0;
    CGFloat scale = _pdfView.scaleFactor;
    BOOL autoScale = _pdfView.autoScales;
    _pdfView.document = doc;
    if (old && index < doc.pageCount) {
        if (!autoScale) _pdfView.scaleFactor = scale;
        PDFDestination *d = [[PDFDestination alloc] initWithPage:[doc pageAtIndex:index] atPoint:dest.point];
        [_pdfView goToDestination:d];
    }
}

// MARK: SyncTeX

- (IBAction)syncToPDF:(id)sender {
    [self syncFromSourceLine:[_textView lineForIndex:_textView.selectedRange.location]];
}

- (void)syncFromSourceLine:(NSUInteger)line {
    if (!_pdfPath || !self.fileURL) {
        NSBeep();
        return;
    }
    NSUInteger hint = 0;
    if (_pdfView.currentPage) hint = [_pdfView.document indexForPage:_pdfView.currentPage] + 1;
    NSString *err = nil;
    NSDictionary *r = rt::dict(rtex_synctex_forward(_pdfPath.UTF8String, self.fileURL.path.UTF8String,
                                                    (uint32_t)line, (uint32_t)hint),
                               &err);
    if (!r) {
        [self appendConsole:[NSString stringWithFormat:@"SyncTeX: %@\n", err] color:NSColor.secondaryLabelColor];
        NSBeep();
        return;
    }
    NSMutableArray<NSValue *> *rects = [NSMutableArray array];
    for (NSDictionary *b in r[@"rects"]) {
        [rects addObject:[NSValue valueWithRect:NSMakeRect([b[@"x"] doubleValue], [b[@"y"] doubleValue],
                                                           [b[@"width"] doubleValue], [b[@"height"] doubleValue])]];
    }
    [_pdfView showRects:rects onPage:[r[@"page"] unsignedIntegerValue]];
}

- (void)syncFromPDFPage:(NSUInteger)page x:(CGFloat)x y:(CGFloat)y {
    if (!_pdfPath) return;
    NSString *err = nil;
    NSDictionary *r = rt::dict(rtex_synctex_inverse(_pdfPath.UTF8String, (uint32_t)page, x, y), &err);
    if (!r) {
        NSBeep();
        return;
    }
    [self openFile:r[@"file"] atLine:[r[@"line"] unsignedIntegerValue]];
}

- (void)openFile:(NSString *)file atLine:(NSUInteger)line {
    if (!file.length || [file.stringByStandardizingPath isEqual:self.fileURL.path.stringByStandardizingPath]) {
        [self selectLine:line];
        return;
    }
    NSURL *url = [NSURL fileURLWithPath:file];
    [NSDocumentController.sharedDocumentController
        openDocumentWithContentsOfURL:url
                              display:YES
                    completionHandler:^(NSDocument *doc, BOOL wasOpen, NSError *error) {
                        if ([doc isKindOfClass:RTDocument.class]) [(RTDocument *)doc selectLine:line];
                    }];
}

- (void)selectLine:(NSUInteger)line {
    [self.windowControllers.firstObject.window makeKeyAndOrderFront:nil];
    [_textView selectLine:line];
}

// MARK: Editing commands

- (IBAction)toggleComment:(id)sender {
    [_textView toggleComment:sender];
}
- (IBAction)shiftRight:(id)sender {
    [_textView shiftRight:sender];
}
- (IBAction)shiftLeft:(id)sender {
    [_textView shiftLeft:sender];
}

- (void)close {
    if (_cancel) rtex_cancel_set(_cancel);
    [self stopWatching];
    [super close];
}

@end
