#import "RTJobs.h"
#import <PDFKit/PDFKit.h>
#import <objc/runtime.h>
#include "RTCore.h"

NSString *const RTDefaultsJobsRoot = @"RTJobsRoot";
NSString *const RTDefaultsJobsAnalysis = @"RTJobsWriteAnalysis";

static NSToolbarItemIdentifier const kNewItem = @"jobs.new";
static NSToolbarItemIdentifier const kTailorItem = @"jobs.tailor";
static NSToolbarItemIdentifier const kTailorInboxItem = @"jobs.tailorInbox";
static NSToolbarItemIdentifier const kMoveItem = @"jobs.move";
static NSToolbarItemIdentifier const kApplyItem = @"jobs.apply";
static NSToolbarItemIdentifier const kRevealItem = @"jobs.reveal";
static NSToolbarItemIdentifier const kEditItem = @"jobs.edit";
static NSToolbarItemIdentifier const kRefreshItem = @"jobs.refresh";
static NSToolbarItemIdentifier const kDeleteItem = @"jobs.delete";

static NSDictionary<NSString *, NSString *> *StageTitles(void) {
    return @{
        @"inbox" : @"Inbox",
        @"apply-now" : @"Ready to Apply",
        @"hold-referral" : @"Hold for Referral",
        @"applied" : @"Applied",
        @"do-not-apply" : @"Do Not Apply",
    };
}

static NSString *Str(id v) {
    return [v isKindOfClass:NSString.class] ? v : @"";
}

static void TailorProgress(void *ctx, const char *eventJSON);

@interface RTJobsController ()
- (void)tailorProgress:(NSString *)message slug:(NSString *)slug;
- (void)liveEdit:(NSDictionary *)edit index:(NSUInteger)index slug:(NSString *)slug;
@end

// Same layout as rtex-jobs' edit_markdown, for edits arriving live.
static NSString *EditMarkdown(NSDictionary *e) {
    NSString *op = Str(e[@"op"]);
    NSString *heading = [NSString stringWithFormat:@"### %@: %@", Str(e[@"section"]), Str(e[@"entry"])];
    NSString *(^code)(NSString *) = ^NSString *(NSString *s) {
        return [NSString stringWithFormat:@"```latex\n%@\n```\n", [s stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceAndNewlineCharacterSet]];
    };
    if ([op isEqual:@"delete"]) return [NSString stringWithFormat:@"%@ (removed)\nRemoved:\n%@%@\n", heading, code(Str(e[@"original"])), Str(e[@"rationale"])];
    if ([op isEqual:@"insert_after"]) return [NSString stringWithFormat:@"%@ (added)\nAdded:\n%@%@\n", heading, code(Str(e[@"replacement"])), Str(e[@"rationale"])];
    return [NSString stringWithFormat:@"%@\nOriginal:\n%@Replacement:\n%@%@\n", heading, code(Str(e[@"original"])), code(Str(e[@"replacement"])), Str(e[@"rationale"])];
}

@implementation RTJobsController {
    NSTableView *_list;
    NSArray<NSDictionary *> *_rows;  // {group: stage} or a job
    NSArray<NSString *> *_stages;
    NSTabView *_tabs;
    NSTextView *_changes;  // edits + checks (from .tailor.json, or streaming)
    NSTextView *_page;     // page.txt
    PDFView *_tailoredPDF;
    PDFView *_basePDF;
    NSTextField *_tailoredLabel;
    NSTableView *_contextTable;
    NSArray<NSDictionary *> *_contextFiles;
    NSMutableSet<NSString *> *_contextSelected;
    NSTextField *_contextSummary;
    NSTextField *_status;
    NSProgressIndicator *_spinner;
    NSPopUpButton *_movePopup;
    dispatch_queue_t _queue;  // tailoring runs one job at a time
    NSMutableOrderedSet<NSString *> *_running;
    NSString *_activeSlug;
    NSString *_root;
    NSMutableDictionary<NSString *, NSMutableArray<NSDictionary *> *> *_live;  // slug → edits streamed so far
    BOOL _pageShownForMissingPDF;  // we left the Resume tab only because there was no PDF
}

+ (instancetype)shared {
    static RTJobsController *c;
    static dispatch_once_t once;
    dispatch_once(&once, ^{ c = [[RTJobsController alloc] init]; });
    return c;
}

- (instancetype)init {
    NSWindow *w = [[NSWindow alloc] initWithContentRect:NSMakeRect(0, 0, 1280, 820)
                                              styleMask:NSWindowStyleMaskTitled | NSWindowStyleMaskClosable |
                                                        NSWindowStyleMaskMiniaturizable | NSWindowStyleMaskResizable
                                                backing:NSBackingStoreBuffered
                                                  defer:NO];
    if ((self = [super initWithWindow:w])) {
        w.title = @"Jobs";
        w.minSize = NSMakeSize(900, 520);
        w.delegate = self;
        w.frameAutosaveName = @"RTJobsWindow";
        _queue = dispatch_queue_create("rtex.jobs.tailor", DISPATCH_QUEUE_SERIAL);
        _running = [NSMutableOrderedSet orderedSet];
        _contextSelected = [NSMutableSet set];
        _live = [NSMutableDictionary dictionary];
        [self buildInterface];
        [self reload];
    }
    return self;
}

- (NSString *)root {
    NSString *r = [NSUserDefaults.standardUserDefaults stringForKey:RTDefaultsJobsRoot];
    if (r.length) return r;
    rt::OwnedString d(rtex_jobs_default_root());
    return d.p ? @(d.p) : NSHomeDirectory();
}

- (IBAction)showJobs:(id)sender {
    [self showWindow:sender];
    [self.window makeKeyAndOrderFront:sender];
    [self reload];
}

- (void)windowDidBecomeKey:(NSNotification *)n {
    [self reload];
}

// MARK: Interface

- (NSScrollView *)scrollingTextView:(NSTextView *__strong *)out {
    NSScrollView *sv = [[NSScrollView alloc] initWithFrame:NSMakeRect(0, 0, 700, 600)];
    sv.hasVerticalScroller = YES;
    sv.borderType = NSNoBorder;
    NSTextView *tv = [[NSTextView alloc] initWithFrame:NSMakeRect(0, 0, 700, 600)];
    tv.editable = NO;
    tv.richText = YES;
    tv.verticallyResizable = YES;
    tv.autoresizingMask = NSViewWidthSizable;
    tv.textContainer.widthTracksTextView = YES;
    tv.textContainerInset = NSMakeSize(18, 14);
    sv.documentView = tv;
    *out = tv;
    return sv;
}

- (void)buildInterface {
    NSWindow *w = self.window;

    // Job list.
    NSScrollView *listScroll = [[NSScrollView alloc] initWithFrame:NSMakeRect(0, 0, 320, 700)];
    listScroll.hasVerticalScroller = YES;
    listScroll.borderType = NSNoBorder;
    _list = [[NSTableView alloc] initWithFrame:listScroll.bounds];
    NSTableColumn *col = [[NSTableColumn alloc] initWithIdentifier:@"job"];
    col.resizingMask = NSTableColumnAutoresizingMask;
    [_list addTableColumn:col];
    _list.headerView = nil;
    _list.columnAutoresizingStyle = NSTableViewFirstColumnOnlyAutoresizingStyle;
    col.width = 300;
    _list.style = NSTableViewStyleSourceList;
    _list.rowSizeStyle = NSTableViewRowSizeStyleCustom;
    _list.dataSource = self;
    _list.delegate = self;
    _list.target = self;
    _list.doubleAction = @selector(openApplicationPage:);
    listScroll.documentView = _list;
    [listScroll.widthAnchor constraintGreaterThanOrEqualToConstant:280].active = YES;

    // Detail tabs.
    _tabs = [[NSTabView alloc] initWithFrame:NSMakeRect(0, 0, 900, 700)];
    NSTabViewItem *resumeTab = [[NSTabViewItem alloc] initWithIdentifier:@"resume"];
    resumeTab.label = @"Resume";
    resumeTab.view = [self resumeView];
    NSTabViewItem *changesTab = [[NSTabViewItem alloc] initWithIdentifier:@"changes"];
    changesTab.label = @"Changes";
    changesTab.view = [self scrollingTextView:&_changes];
    NSTabViewItem *pageTab = [[NSTabViewItem alloc] initWithIdentifier:@"page"];
    pageTab.label = @"Page";
    pageTab.view = [self scrollingTextView:&_page];
    NSTabViewItem *contextTab = [[NSTabViewItem alloc] initWithIdentifier:@"context"];
    contextTab.label = @"Context";
    contextTab.view = [self contextView];
    for (NSTabViewItem *t in @[ resumeTab, changesTab, pageTab, contextTab ]) [_tabs addTabViewItem:t];

    NSSplitView *split = [[NSSplitView alloc] initWithFrame:NSMakeRect(0, 28, 1280, 792)];
    split.vertical = YES;
    split.dividerStyle = NSSplitViewDividerStyleThin;
    [split addArrangedSubview:listScroll];
    [split addArrangedSubview:_tabs];
    [split setHoldingPriority:NSLayoutPriorityDefaultLow + 1 forSubviewAtIndex:0];

    // Status bar.
    _status = [NSTextField labelWithString:@""];
    _status.font = [NSFont systemFontOfSize:NSFont.smallSystemFontSize];
    _status.textColor = NSColor.secondaryLabelColor;
    _status.lineBreakMode = NSLineBreakByTruncatingTail;
    _spinner = [[NSProgressIndicator alloc] init];
    _spinner.style = NSProgressIndicatorStyleSpinning;
    _spinner.controlSize = NSControlSizeSmall;
    _spinner.displayedWhenStopped = NO;
    NSStackView *statusBar = [NSStackView stackViewWithViews:@[ _spinner, _status ]];
    statusBar.edgeInsets = NSEdgeInsetsMake(4, 12, 4, 12);

    NSStackView *main = [NSStackView stackViewWithViews:@[ split, statusBar ]];
    main.orientation = NSUserInterfaceLayoutOrientationVertical;
    main.spacing = 0;
    main.alignment = NSLayoutAttributeLeading;
    [split.widthAnchor constraintEqualToAnchor:main.widthAnchor].active = YES;
    [statusBar.widthAnchor constraintEqualToAnchor:main.widthAnchor].active = YES;
    w.contentView = main;
    [w layoutIfNeeded];
    [split setPosition:330 ofDividerAtIndex:0];

    NSToolbar *tb = [[NSToolbar alloc] initWithIdentifier:@"RTJobsToolbar"];
    tb.delegate = self;
    tb.displayMode = NSToolbarDisplayModeIconAndLabel;
    w.toolbar = tb;
    w.toolbarStyle = NSWindowToolbarStyleExpanded;
    [w center];
}

- (NSView *)resumeView {
    _tailoredPDF = [[PDFView alloc] init];
    _basePDF = [[PDFView alloc] init];
    for (PDFView *p in @[ _tailoredPDF, _basePDF ]) {
        p.autoScales = YES;
        p.displayMode = kPDFDisplaySinglePageContinuous;
    }
    _tailoredLabel = [NSTextField labelWithString:@"Tailored"];
    NSTextField *baseLabel = [NSTextField labelWithString:@"base.pdf"];
    for (NSTextField *l in @[ _tailoredLabel, baseLabel ]) {
        l.font = [NSFont boldSystemFontOfSize:NSFont.smallSystemFontSize];
        l.textColor = NSColor.secondaryLabelColor;
        l.lineBreakMode = NSLineBreakByTruncatingMiddle;
        [l setContentCompressionResistancePriority:NSLayoutPriorityDefaultLow forOrientation:NSLayoutConstraintOrientationHorizontal];
    }
    NSStackView *(^column)(NSTextField *, PDFView *) = ^NSStackView *(NSTextField *label, PDFView *pdf) {
        NSStackView *s = [NSStackView stackViewWithViews:@[ label, pdf ]];
        s.orientation = NSUserInterfaceLayoutOrientationVertical;
        s.alignment = NSLayoutAttributeLeading;
        s.edgeInsets = NSEdgeInsetsMake(6, 0, 0, 0);
        s.spacing = 4;
        [pdf.widthAnchor constraintEqualToAnchor:s.widthAnchor].active = YES;
        return s;
    };
    NSSplitView *split = [[NSSplitView alloc] initWithFrame:NSMakeRect(0, 0, 900, 700)];
    split.vertical = YES;
    split.dividerStyle = NSSplitViewDividerStyleThin;
    NSStackView *left = column(_tailoredLabel, _tailoredPDF);
    NSStackView *right = column(baseLabel, _basePDF);
    [split addArrangedSubview:left];
    [split addArrangedSubview:right];
    // Side by side, same width: the tailored resume next to the base it came from.
    [left.widthAnchor constraintEqualToAnchor:right.widthAnchor].active = YES;
    return split;
}

- (NSView *)contextView {
    NSScrollView *sv = [[NSScrollView alloc] initWithFrame:NSMakeRect(0, 0, 900, 600)];
    sv.hasVerticalScroller = YES;
    _contextTable = [[NSTableView alloc] initWithFrame:sv.bounds];
    for (NSArray *c in @[ @[ @"use", @"", @40 ], @[ @"path", @"File", @420 ], @[ @"size", @"Size", @70 ], @[ @"note", @"", @300 ] ]) {
        NSTableColumn *col = [[NSTableColumn alloc] initWithIdentifier:c[0]];
        col.title = c[1];
        col.width = [c[2] doubleValue];
        [_contextTable addTableColumn:col];
    }
    _contextTable.dataSource = self;
    _contextTable.delegate = self;
    _contextTable.usesAlternatingRowBackgroundColors = YES;
    sv.documentView = _contextTable;
    _contextSummary = [NSTextField wrappingLabelWithString:@""];
    _contextSummary.textColor = NSColor.secondaryLabelColor;
    NSButton *save = [NSButton buttonWithTitle:@"Save Selection" target:self action:@selector(saveContext:)];
    NSButton *defaults = [NSButton buttonWithTitle:@"Use Defaults" target:self action:@selector(resetContext:)];
    NSStackView *bar = [NSStackView stackViewWithViews:@[ _contextSummary, defaults, save ]];
    bar.edgeInsets = NSEdgeInsetsMake(8, 12, 8, 12);
    [_contextSummary setContentHuggingPriority:NSLayoutPriorityDefaultLow forOrientation:NSLayoutConstraintOrientationHorizontal];
    NSStackView *s = [NSStackView stackViewWithViews:@[ sv, bar ]];
    s.orientation = NSUserInterfaceLayoutOrientationVertical;
    s.spacing = 0;
    [sv.widthAnchor constraintEqualToAnchor:s.widthAnchor].active = YES;
    [bar.widthAnchor constraintEqualToAnchor:s.widthAnchor].active = YES;
    return s;
}

// MARK: Toolbar

- (NSArray<NSToolbarItemIdentifier> *)toolbarDefaultItemIdentifiers:(NSToolbar *)toolbar {
    return @[ kNewItem, kTailorItem, kTailorInboxItem, NSToolbarSpaceItemIdentifier, kMoveItem, kApplyItem, kRevealItem,
              kEditItem, NSToolbarFlexibleSpaceItemIdentifier, kDeleteItem, kRefreshItem ];
}

- (NSArray<NSToolbarItemIdentifier> *)toolbarAllowedItemIdentifiers:(NSToolbar *)toolbar {
    return [self toolbarDefaultItemIdentifiers:toolbar];
}

- (NSToolbarItem *)toolbar:(NSToolbar *)toolbar itemForItemIdentifier:(NSToolbarItemIdentifier)ident willBeInsertedIntoToolbar:(BOOL)flag {
    NSToolbarItem *item = [[NSToolbarItem alloc] initWithItemIdentifier:ident];
    item.target = self;
    NSArray *spec = @{
        kNewItem : @[ @"New Job", @"plus", @"newJob:", @"Add a posting from a URL or pasted text" ],
        kTailorItem : @[ @"Tailor", @"wand.and.stars", @"tailorSelected:", @"Tailor the resume for the selected job" ],
        kTailorInboxItem : @[ @"Tailor Inbox", @"tray.full", @"tailorInbox:", @"Tailor every job in the inbox, one after another" ],
        kApplyItem : @[ @"Apply Page", @"safari", @"openApplicationPage:", @"Open the application page in your browser" ],
        kRevealItem : @[ @"Show PDF", @"doc.richtext", @"revealResume:", @"Show the PDF to upload in Finder" ],
        kEditItem : @[ @"Edit Resume", @"pencil", @"editResume:", @"Open the tailored .tex in RTeX" ],
        kRefreshItem : @[ @"Refresh", @"arrow.clockwise", @"reload", @"Re-read the job folders" ],
        kDeleteItem : @[ @"Delete", @"trash", @"deleteJob:", @"Move the job's folder to the Trash" ],
    }[ident];
    if (spec) {
        item.label = spec[0];
        item.image = [NSImage imageWithSystemSymbolName:spec[1] accessibilityDescription:spec[0]];
        item.action = NSSelectorFromString(spec[2]);
        item.toolTip = spec[3];
    } else if ([ident isEqual:kMoveItem]) {
        item.label = @"Move To";
        _movePopup = [[NSPopUpButton alloc] initWithFrame:NSMakeRect(0, 0, 170, 26) pullsDown:YES];
        [_movePopup addItemWithTitle:@"Move To"];
        for (NSString *stage in @[ @"inbox", @"apply-now", @"hold-referral", @"applied", @"do-not-apply" ]) {
            [_movePopup addItemWithTitle:StageTitles()[stage]];
            _movePopup.lastItem.representedObject = stage;
            _movePopup.lastItem.target = self;
            _movePopup.lastItem.action = @selector(moveSelected:);
        }
        item.view = _movePopup;
    }
    return item;
}

- (BOOL)validateToolbarItem:(NSToolbarItem *)item {
    NSDictionary *job = [self selectedJob];
    SEL a = item.action;
    if (a == @selector(tailorSelected:)) return job && ![_running containsObject:job[@"slug"]];
    if (a == @selector(openApplicationPage:)) return job && (Str(job[@"apply_url"]).length || Str(job[@"source"]).length);
    if (a == @selector(revealResume:)) return [job[@"resume_pdf"] isKindOfClass:NSString.class];
    if (a == @selector(editResume:)) return [self tailoredTexFor:job] != nil;
    if (a == @selector(deleteJob:)) return job && ![_running containsObject:job[@"slug"]];
    return YES;
}

// MARK: Data

- (void)reload {
    rt::OwnedString raw(rtex_jobs_list(self.root.UTF8String));
    NSDictionary *d = raw.p ? [NSJSONSerialization JSONObjectWithData:[NSData dataWithBytes:raw.p length:strlen(raw.p)]
                                                             options:0
                                                               error:nil]
                            : nil;
    NSString *selected = [self selectedJob][@"slug"];
    if (!d || d[@"error"]) {
        _rows = @[];
        [_list reloadData];
        [self setStatus:[NSString stringWithFormat:@"No job folder at %@ (%@). Jobs → Choose Job Folder… to pick one.",
                                                   self.root, d[@"error"] ?: @"unreadable"]];
        return;
    }
    _root = d[@"root"];
    _stages = d[@"stages"];
    NSMutableArray *rows = [NSMutableArray array];
    for (NSString *stage in _stages) {
        NSArray *jobs = [d[@"jobs"] filteredArrayUsingPredicate:[NSPredicate predicateWithFormat:@"stage == %@", stage]];
        if (!jobs.count && ![stage isEqual:@"inbox"]) continue;
        [rows addObject:@{@"group" : stage, @"count" : @(jobs.count)}];
        [rows addObjectsFromArray:jobs];
    }
    _rows = rows;
    [_list reloadData];
    NSInteger idx = [self rowForSlug:selected];
    if (idx < 0) {
        for (NSUInteger i = 0; i < _rows.count; i++) {
            if (!_rows[i][@"group"]) { idx = (NSInteger)i; break; }
        }
    }
    if (idx >= 0) [_list selectRowIndexes:[NSIndexSet indexSetWithIndex:(NSUInteger)idx] byExtendingSelection:NO];
    [self showSelection];
    if (!_running.count) {
        NSUInteger n = [d[@"jobs"] count];
        [self setStatus:[NSString stringWithFormat:@"%lu job%@ in %@", (unsigned long)n, n == 1 ? @"" : @"s",
                                                   [_root stringByAbbreviatingWithTildeInPath]]];
    }
}

- (NSInteger)rowForSlug:(NSString *)slug {
    if (!slug) return -1;
    for (NSUInteger i = 0; i < _rows.count; i++) {
        if ([_rows[i][@"slug"] isEqual:slug]) return (NSInteger)i;
    }
    return -1;
}

- (NSDictionary *)selectedJob {
    NSInteger r = _list.selectedRow;
    if (r < 0 || r >= (NSInteger)_rows.count) return nil;
    NSDictionary *row = _rows[(NSUInteger)r];
    return row[@"group"] ? nil : row;
}

- (NSString *)jobDir:(NSDictionary *)job {
    return Str(job[@"dir"]);
}

- (NSDictionary *)tailorInfo:(NSDictionary *)job {
    NSData *data = [NSData dataWithContentsOfFile:[[self jobDir:job] stringByAppendingPathComponent:@".tailor.json"]];
    id v = data ? [NSJSONSerialization JSONObjectWithData:data options:0 error:nil] : nil;
    return [v isKindOfClass:NSDictionary.class] ? v : nil;
}

- (NSString *)tailoredTexFor:(NSDictionary *)job {
    NSString *tex = Str([self tailorInfo:job][@"tex"]);
    return tex.length && [NSFileManager.defaultManager fileExistsAtPath:tex] ? tex : nil;
}

// Keep attachments/<name>.pdf in step with the tailored PDF after manual edits.
- (void)syncAttachment:(NSDictionary *)job {
    NSString *att = Str(job[@"resume_pdf"]);
    NSString *pdf = Str([self tailorInfo:job][@"pdf"]);
    if (!att.length || !pdf.length) return;
    NSFileManager *fm = NSFileManager.defaultManager;
    NSDate *a = [fm attributesOfItemAtPath:att error:nil].fileModificationDate;
    NSDate *p = [fm attributesOfItemAtPath:pdf error:nil].fileModificationDate;
    if (p && a && [p compare:a] == NSOrderedDescending) {
        [fm removeItemAtPath:att error:nil];
        [fm copyItemAtPath:pdf toPath:att error:nil];
    }
}

// MARK: Table

- (NSInteger)numberOfRowsInTableView:(NSTableView *)tv {
    return tv == _list ? (NSInteger)_rows.count : (NSInteger)_contextFiles.count;
}

- (BOOL)tableView:(NSTableView *)tv isGroupRow:(NSInteger)row {
    return tv == _list && _rows[(NSUInteger)row][@"group"] != nil;
}

- (BOOL)tableView:(NSTableView *)tv shouldSelectRow:(NSInteger)row {
    return tv != _list || !_rows[(NSUInteger)row][@"group"];
}

- (CGFloat)tableView:(NSTableView *)tv heightOfRow:(NSInteger)row {
    if (tv != _list) return 22;
    return _rows[(NSUInteger)row][@"group"] ? 26 : 50;
}

- (NSView *)tableView:(NSTableView *)tv viewForTableColumn:(NSTableColumn *)column row:(NSInteger)row {
    if (tv == _contextTable) return [self contextCell:column row:row];
    NSDictionary *r = _rows[(NSUInteger)row];
    if (r[@"group"]) {
        NSTextField *t = [NSTextField labelWithString:[NSString stringWithFormat:@"%@  %@", StageTitles()[r[@"group"]] ?: r[@"group"], r[@"count"]]];
        t.font = [NSFont boldSystemFontOfSize:NSFont.smallSystemFontSize];
        t.textColor = NSColor.secondaryLabelColor;
        return t;
    }
    NSTextField *title = [NSTextField labelWithString:Str(r[@"title"]).length ? r[@"title"] : r[@"slug"]];
    title.font = [NSFont systemFontOfSize:13 weight:NSFontWeightMedium];
    title.lineBreakMode = NSLineBreakByTruncatingTail;
    NSString *status = @"";
    if ([_running containsObject:r[@"slug"]]) {
        status = [r[@"slug"] isEqual:_activeSlug] ? @"  · tailoring…" : @"  · queued";
    } else if ([r[@"resume_pdf"] isKindOfClass:NSString.class]) {
        status = @"  · PDF ready";
    } else {
        status = @"  · not tailored";
    }
    NSString *sub = Str(r[@"company"]);
    if (!sub.length) sub = [NSURL URLWithString:Str(r[@"source"])].host ?: @"pasted text";
    NSTextField *subtitle = [NSTextField labelWithString:[sub stringByAppendingString:status]];
    subtitle.font = [NSFont systemFontOfSize:NSFont.smallSystemFontSize];
    subtitle.textColor = NSColor.secondaryLabelColor;
    subtitle.lineBreakMode = NSLineBreakByTruncatingTail;
    NSStackView *s = [NSStackView stackViewWithViews:@[ title, subtitle ]];
    s.orientation = NSUserInterfaceLayoutOrientationVertical;
    s.alignment = NSLayoutAttributeLeading;
    s.spacing = 2;
    s.edgeInsets = NSEdgeInsetsMake(6, 4, 6, 4);
    for (NSView *v in @[ title, subtitle ]) {
        [v setContentCompressionResistancePriority:NSLayoutPriorityDefaultLow forOrientation:NSLayoutConstraintOrientationHorizontal];
    }
    return s;
}

- (void)tableViewSelectionDidChange:(NSNotification *)n {
    if (n.object == _list) [self showSelection];
}

- (void)showSelection {
    NSDictionary *job = [self selectedJob];
    [self.window.toolbar validateVisibleItems];
    if (!job) {
        _changes.string = @"";
        _page.string = @"";
        _tailoredPDF.document = nil;
        _contextFiles = @[];
        [_contextTable reloadData];
        return;
    }
    [self syncAttachment:job];
    if (_live[job[@"slug"]]) {  // still streaming: show the edits so far
        [self showLive:job[@"slug"]];
        [self loadContext:job];
        return;
    }
    NSString *summary = Str([self tailorInfo:job][@"summary"]);
    [_changes.textStorage setAttributedString:[RTJobsController renderMarkdown:summary.length ? summary : @"Not tailored yet. Click **Tailor**."]];
    NSString *page = [NSString stringWithContentsOfFile:Str(job[@"page_file"]) encoding:NSUTF8StringEncoding error:nil] ?: @"";
    [_page.textStorage setAttributedString:[[NSAttributedString alloc] initWithString:page attributes:@{
        NSFontAttributeName : [NSFont systemFontOfSize:12.5], NSForegroundColorAttributeName : NSColor.labelColor}]];

    NSString *pdf = Str(job[@"resume_pdf"]);
    _tailoredPDF.document = pdf.length ? [[PDFDocument alloc] initWithURL:[NSURL fileURLWithPath:pdf]] : nil;
    _tailoredLabel.stringValue = pdf.length ? [NSString stringWithFormat:@"Tailored: %@", pdf.lastPathComponent] : @"Tailored: not built yet";
    // Resume first; jobs without a PDF yet open on their page text.
    NSString *tab = _tabs.selectedTabViewItem.identifier;
    if (!pdf.length && [tab isEqual:@"resume"]) {
        [_tabs selectTabViewItemWithIdentifier:@"page"];
        _pageShownForMissingPDF = YES;
    } else if (pdf.length && _pageShownForMissingPDF && [tab isEqual:@"page"]) {
        [_tabs selectTabViewItemWithIdentifier:@"resume"];
        _pageShownForMissingPDF = NO;
    }
    if (!_basePDF.document && _root) {
        _basePDF.document = [[PDFDocument alloc] initWithURL:[NSURL fileURLWithPath:[_root stringByAppendingPathComponent:@"resumes/base.pdf"]]];
    }
    [self loadContext:job];
}

// MARK: Context tab

- (void)loadContext:(NSDictionary *)job {
    NSDictionary *c = rt::dict(rtex_jobs_context(_root.UTF8String, Str(job[@"slug"]).UTF8String));
    _contextFiles = c[@"files"] ?: @[];
    _contextSelected = [NSMutableSet setWithArray:c[@"selected"] ?: @[]];
    [_contextTable reloadData];
    [self updateContextSummary];
}

- (void)updateContextSummary {
    unsigned long long bytes = 0;
    for (NSDictionary *f in _contextFiles) {
        if ([_contextSelected containsObject:f[@"path"]]) bytes += [f[@"bytes"] unsignedLongLongValue];
    }
    _contextSummary.stringValue = [NSString stringWithFormat:@"%lu files, about %llu K tokens sent to Claude for this job.",
                                                             (unsigned long)_contextSelected.count, bytes / 4000];
}

- (NSView *)contextCell:(NSTableColumn *)column row:(NSInteger)row {
    NSDictionary *f = _contextFiles[(NSUInteger)row];
    NSString *ident = column.identifier;
    if ([ident isEqual:@"use"]) {
        NSButton *b = [NSButton checkboxWithTitle:@"" target:self action:@selector(toggleContext:)];
        b.state = [_contextSelected containsObject:f[@"path"]] ? NSControlStateValueOn : NSControlStateValueOff;
        b.tag = row;
        b.enabled = ![f[@"path"] isEqual:@"resumes/base.tex"];
        return b;
    }
    NSString *text = [ident isEqual:@"path"] ? f[@"path"]
                   : [ident isEqual:@"size"] ? [NSString stringWithFormat:@"%.1f KB", [f[@"bytes"] doubleValue] / 1024.0]
                                              : f[@"note"];
    NSTextField *t = [NSTextField labelWithString:text ?: @""];
    t.lineBreakMode = NSLineBreakByTruncatingMiddle;
    if (![ident isEqual:@"path"]) t.textColor = NSColor.secondaryLabelColor;
    return t;
}

- (void)toggleContext:(NSButton *)sender {
    NSString *path = _contextFiles[(NSUInteger)sender.tag][@"path"];
    if (sender.state == NSControlStateValueOn) [_contextSelected addObject:path];
    else [_contextSelected removeObject:path];
    [self updateContextSummary];
}

- (void)saveContext:(id)sender {
    NSDictionary *job = [self selectedJob];
    if (!job) return;
    NSMutableArray *ordered = [NSMutableArray array];
    for (NSDictionary *f in _contextFiles) {
        if ([_contextSelected containsObject:f[@"path"]]) [ordered addObject:f[@"path"]];
    }
    NSData *json = [NSJSONSerialization dataWithJSONObject:ordered options:0 error:nil];
    NSString *s = [[NSString alloc] initWithData:json encoding:NSUTF8StringEncoding];
    rt::dict(rtex_jobs_set_context(_root.UTF8String, Str(job[@"slug"]).UTF8String, s.UTF8String));
    [self setStatus:@"Context saved for this job."];
}

- (void)resetContext:(id)sender {
    NSDictionary *job = [self selectedJob];
    if (!job) return;
    [NSFileManager.defaultManager removeItemAtPath:[[self jobDir:job] stringByAppendingPathComponent:@"context.txt"] error:nil];
    [self loadContext:job];
    [self setStatus:@"Using the default context for this job."];
}

// MARK: Actions

- (void)setStatus:(NSString *)s {
    _status.stringValue = s ?: @"";
}

- (void)fail:(NSString *)what error:(NSString *)error {
    NSAlert *a = [[NSAlert alloc] init];
    a.messageText = what;
    a.informativeText = error ?: @"Unknown error";
    [a beginSheetModalForWindow:self.window completionHandler:nil];
}

- (IBAction)tailorSelected:(id)sender {
    NSDictionary *job = [self selectedJob];
    if (job) [self enqueueTailor:job];
}

- (IBAction)tailorInbox:(id)sender {
    NSUInteger n = 0;
    for (NSDictionary *r in _rows) {
        if ([r[@"stage"] isEqual:@"inbox"] && ![r[@"resume_pdf"] isKindOfClass:NSString.class]) {
            [self enqueueTailor:r];
            n++;
        }
    }
    if (!n) [self setStatus:@"Nothing to tailor: every inbox job already has a review."];
}

- (void)enqueueTailor:(NSDictionary *)job {
    NSString *slug = job[@"slug"];
    if ([_running containsObject:slug]) return;
    [_running addObject:slug];
    [_spinner startAnimation:nil];
    [_list reloadData];
    [self.window.toolbar validateVisibleItems];
    NSString *root = _root;
    NSString *label = Str(job[@"company"]).length ? job[@"company"] : slug;
    dispatch_async(_queue, ^{
        dispatch_sync(dispatch_get_main_queue(), ^{
            self->_activeSlug = slug;
            [self setStatus:[NSString stringWithFormat:@"%@: starting…", label]];
            [self->_list reloadData];
        });
        // Precise lifetime: the C callback uses ctx until rtex_jobs_tailor returns.
        __attribute__((objc_precise_lifetime)) NSDictionary *ctx = @{@"controller" : self, @"slug" : slug};
        BOOL analysis = [NSUserDefaults.standardUserDefaults boolForKey:RTDefaultsJobsAnalysis];
        NSString *opts = analysis ? @"{\"analysis\":true}" : @"{\"analysis\":false}";
        char *raw = rtex_jobs_tailor(root.UTF8String, slug.UTF8String, opts.UTF8String, true, TailorProgress, (__bridge void *)ctx);
        NSString *err = nil;
        NSDictionary *out = rt::dict(raw, &err);
        dispatch_async(dispatch_get_main_queue(), ^{ [self finishTailor:slug label:label outcome:out error:err]; });
    });
}

static void TailorProgress(void *ctx, const char *eventJSON) {
    NSDictionary *c = (__bridge NSDictionary *)ctx;
    NSData *data = [NSData dataWithBytes:eventJSON length:strlen(eventJSON)];
    NSDictionary *ev = [NSJSONSerialization JSONObjectWithData:data options:0 error:nil];
    if (![ev isKindOfClass:NSDictionary.class]) return;
    RTJobsController *controller = c[@"controller"];
    NSString *slug = c[@"slug"];
    dispatch_async(dispatch_get_main_queue(), ^{
        if ([ev[@"kind"] isEqual:@"edit"]) {
            [controller liveEdit:ev[@"edit"] index:[ev[@"index"] unsignedIntegerValue] slug:slug];
        } else {
            [controller tailorProgress:Str(ev[@"text"]) slug:slug];
        }
    });
}

// An edit just finished streaming: show it while the rest is still coming.
- (void)liveEdit:(NSDictionary *)edit index:(NSUInteger)index slug:(NSString *)slug {
    if (![edit isKindOfClass:NSDictionary.class]) return;
    NSMutableArray *edits = _live[slug] ?: (_live[slug] = [NSMutableArray array]);
    if (index < edits.count) edits[index] = edit;
    else [edits addObject:edit];
    [self showLive:slug];
}

- (void)showLive:(NSString *)slug {
    NSDictionary *job = [self selectedJob];
    NSArray *edits = _live[slug];
    if (![job[@"slug"] isEqual:slug] || !edits) return;
    NSMutableString *md = [NSMutableString stringWithFormat:@"# %@ - %@\n\n## Tailoring… %lu edit%@ so far\n\n",
                                                            Str(job[@"company"]), Str(job[@"title"]), (unsigned long)edits.count,
                                                            edits.count == 1 ? @"" : @"s"];
    for (NSDictionary *e in edits) {
        [md appendString:EditMarkdown(e)];
        [md appendString:@"\n"];
    }
    [_changes.textStorage setAttributedString:[RTJobsController renderMarkdown:md]];
    [_tabs selectTabViewItemWithIdentifier:@"changes"];
}

- (void)tailorProgress:(NSString *)message slug:(NSString *)slug {
    NSString *label = slug;
    for (NSDictionary *r in _rows) {
        if ([r[@"slug"] isEqual:slug] && Str(r[@"company"]).length) label = r[@"company"];
    }
    NSUInteger waiting = _running.count > 0 ? _running.count - 1 : 0;
    [self setStatus:[NSString stringWithFormat:@"%@: %@%@", label, message,
                                               waiting ? [NSString stringWithFormat:@"  (%lu more queued)", (unsigned long)waiting] : @""]];
}

- (void)finishTailor:(NSString *)slug label:(NSString *)label outcome:(NSDictionary *)out error:(NSString *)err {
    [_running removeObject:slug];
    [_live removeObjectForKey:slug];
    if ([_activeSlug isEqual:slug]) _activeSlug = nil;
    if (!_running.count) [_spinner stopAnimation:nil];
    NSString *newSlug = [out[@"job"] isKindOfClass:NSDictionary.class] ? out[@"job"][@"slug"] : slug;
    [self reload];
    if (!out) {
        [self setStatus:[NSString stringWithFormat:@"%@: failed", label]];
        [self fail:[NSString stringWithFormat:@"Tailoring %@ failed", label] error:err];
        return;
    }
    NSString *summary;
    if ([out[@"decision"] isEqual:@"stop"]) {
        summary = [NSString stringWithFormat:@"%@: stopped (%@)", label, out[@"stop_reason"]];
    } else if ([out[@"ready"] boolValue]) {
        summary = [NSString stringWithFormat:@"%@: resume ready (%lu edits, %.0f s)", label,
                                             (unsigned long)[out[@"edits"] count], [out[@"seconds"] doubleValue]];
    } else {
        summary = [NSString stringWithFormat:@"%@: tailored, needs a look (see Checks in the review)", label];
    }
    if ([out[@"check_claims"] count]) summary = [summary stringByAppendingString:@" · claims to check"];
    [self setStatus:summary];
    NSInteger row = [self rowForSlug:newSlug];
    if (row >= 0 && !_running.count) {
        [_list selectRowIndexes:[NSIndexSet indexSetWithIndex:(NSUInteger)row] byExtendingSelection:NO];
        [_tabs selectTabViewItemWithIdentifier:@"resume"];
    }
}

- (IBAction)moveSelected:(NSMenuItem *)sender {
    NSDictionary *job = [self selectedJob];
    if (!job || !sender.representedObject) return;
    NSString *err = nil;
    NSDictionary *moved = rt::dict(rtex_jobs_move(_root.UTF8String, Str(job[@"slug"]).UTF8String,
                                                  [sender.representedObject UTF8String]),
                                   &err);
    if (!moved) [self fail:@"Could not move the job" error:err];
    [self reload];
}

- (IBAction)deleteJob:(id)sender {
    NSDictionary *job = [self selectedJob];
    if (!job) return;
    NSAlert *a = [[NSAlert alloc] init];
    a.messageText = [NSString stringWithFormat:@"Move “%@” to the Trash?", Str(job[@"title"])];
    a.informativeText = @"Its folder (page text and tailored resume) goes to the Trash; you can put it back from there.";
    [a addButtonWithTitle:@"Move to Trash"];
    [a addButtonWithTitle:@"Cancel"];
    [a beginSheetModalForWindow:self.window completionHandler:^(NSModalResponse r) {
        if (r != NSAlertFirstButtonReturn) return;
        NSError *e = nil;
        if (![NSFileManager.defaultManager trashItemAtURL:[NSURL fileURLWithPath:[self jobDir:job]] resultingItemURL:nil error:&e]) {
            [self fail:@"Could not move the job to the Trash" error:e.localizedDescription];
        }
        [self reload];
    }];
}

- (IBAction)openApplicationPage:(id)sender {
    NSDictionary *job = [self selectedJob];
    NSString *url = Str(job[@"apply_url"]).length ? job[@"apply_url"] : Str(job[@"source"]);
    if (url.length) [NSWorkspace.sharedWorkspace openURL:[NSURL URLWithString:url]];
}

- (IBAction)revealResume:(id)sender {
    NSString *pdf = Str([self selectedJob][@"resume_pdf"]);
    if (pdf.length) [NSWorkspace.sharedWorkspace activateFileViewerSelectingURLs:@[ [NSURL fileURLWithPath:pdf] ]];
}

- (IBAction)editResume:(id)sender {
    NSString *tex = [self tailoredTexFor:[self selectedJob]];
    if (!tex) return;
    [NSDocumentController.sharedDocumentController openDocumentWithContentsOfURL:[NSURL fileURLWithPath:tex]
                                                                         display:YES
                                                               completionHandler:^(NSDocument *d, BOOL open, NSError *e){
                                                               }];
}

- (IBAction)chooseJobsFolder:(id)sender {
    NSOpenPanel *p = [NSOpenPanel openPanel];
    p.canChooseDirectories = YES;
    p.canChooseFiles = NO;
    p.message = @"Choose your job-search folder (it has resumes/base.tex and jobs/).";
    if (_root) p.directoryURL = [NSURL fileURLWithPath:_root];
    [p beginSheetModalForWindow:self.window completionHandler:^(NSModalResponse r) {
        if (r != NSModalResponseOK) return;
        [NSUserDefaults.standardUserDefaults setObject:p.URL.path forKey:RTDefaultsJobsRoot];
        self->_basePDF.document = nil;
        [self reload];
    }];
}

// MARK: New job sheet

- (IBAction)newJob:(id)sender {
    [self showJobs:sender];
    NSWindow *sheet = [[NSWindow alloc] initWithContentRect:NSMakeRect(0, 0, 620, 520)
                                                  styleMask:NSWindowStyleMaskTitled
                                                    backing:NSBackingStoreBuffered
                                                      defer:NO];
    NSTextField *intro = [NSTextField wrappingLabelWithString:
        @"Paste the job page's URL: its text is saved and your resume is tailored from it. If the page needs a sign-in, "
        @"paste the page's text below instead (⌘A, ⌘C on the page)."];
    NSTextField *url = [NSTextField textFieldWithString:@""];
    url.placeholderString = @"https://…";
    NSString *clip = [NSPasteboard.generalPasteboard stringForType:NSPasteboardTypeString];
    if ([clip hasPrefix:@"http"] && ![clip containsString:@"\n"]) url.stringValue = clip;
    NSTextField *title = [NSTextField textFieldWithString:@""];
    title.placeholderString = @"Folder name for pasted text, e.g. Acme Software Engineer (optional)";
    NSTextField *company = [NSTextField textFieldWithString:@""];
    NSTextField *location = [NSTextField textFieldWithString:@""];
    NSStackView *fields = [NSStackView stackViewWithViews:@[ title ]];
    NSScrollView *textScroll = [[NSScrollView alloc] init];
    textScroll.hasVerticalScroller = YES;
    textScroll.borderType = NSBezelBorder;
    NSTextView *text = [[NSTextView alloc] initWithFrame:NSMakeRect(0, 0, 580, 260)];
    text.richText = NO;
    text.font = [NSFont systemFontOfSize:12];
    text.autoresizingMask = NSViewWidthSizable;
    textScroll.documentView = text;
    [textScroll.heightAnchor constraintEqualToConstant:260].active = YES;
    NSButton *tailorNow = [NSButton checkboxWithTitle:@"Tailor right away" target:nil action:nil];
    tailorNow.state = NSControlStateValueOn;
    NSButton *cancel = [NSButton buttonWithTitle:@"Cancel" target:nil action:nil];
    NSButton *add = [NSButton buttonWithTitle:@"Add Job" target:nil action:nil];
    add.keyEquivalent = @"\r";
    cancel.keyEquivalent = @"\033";
    NSStackView *buttons = [NSStackView stackViewWithViews:@[ tailorNow, [[NSView alloc] init], cancel, add ]];
    NSStackView *s = [NSStackView stackViewWithViews:@[ intro, url, fields, textScroll, buttons ]];
    s.orientation = NSUserInterfaceLayoutOrientationVertical;
    s.alignment = NSLayoutAttributeLeading;
    s.spacing = 10;
    s.edgeInsets = NSEdgeInsetsMake(18, 20, 18, 20);
    for (NSView *v in @[ intro, url, fields, textScroll, buttons ]) [v.widthAnchor constraintEqualToAnchor:s.widthAnchor constant:-40].active = YES;
    sheet.contentView = s;

    __weak NSWindow *weakSheet = sheet;
    cancel.target = self;
    cancel.action = @selector(endSheetCancel:);
    add.target = self;
    add.action = @selector(endSheetAdd:);
    objc_setAssociatedObject(sheet, "fields", @{@"url" : url, @"title" : title, @"company" : company, @"location" : location,
                                                @"text" : text, @"tailor" : tailorNow},
                             OBJC_ASSOCIATION_RETAIN_NONATOMIC);
    [self.window beginSheet:sheet completionHandler:^(NSModalResponse r) {
        if (r != NSModalResponseOK) return;
        NSDictionary *f = objc_getAssociatedObject(weakSheet, "fields");
        [self addJobFromFields:f];
    }];
}

- (void)endSheetCancel:(NSButton *)b {
    [self.window endSheet:b.window returnCode:NSModalResponseCancel];
}

- (void)endSheetAdd:(NSButton *)b {
    NSDictionary *f = objc_getAssociatedObject(b.window, "fields");
    NSString *url = [[f[@"url"] stringValue] stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceAndNewlineCharacterSet];
    NSString *text = [f[@"text"] string];
    if (!url.length && !text.length) {
        NSBeep();
        return;
    }
    [self.window endSheet:b.window returnCode:NSModalResponseOK];
}

- (void)addJobFromFields:(NSDictionary *)f {
    NSString *url = [[f[@"url"] stringValue] stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceAndNewlineCharacterSet];
    NSString *text = [[f[@"text"] string] stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceAndNewlineCharacterSet];
    BOOL tailor = [f[@"tailor"] state] == NSControlStateValueOn;
    NSDictionary *fields = @{@"title" : [f[@"title"] stringValue], @"company" : [f[@"company"] stringValue],
                             @"location" : [f[@"location"] stringValue], @"source" : url};
    NSString *root = _root;
    [_spinner startAnimation:nil];
    [self setStatus:text.length ? @"Saving the posting…" : [NSString stringWithFormat:@"Fetching %@…", url]];
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
        char *raw;
        if (text.length) {
            NSData *j = [NSJSONSerialization dataWithJSONObject:fields options:0 error:nil];
            NSString *js = [[NSString alloc] initWithData:j encoding:NSUTF8StringEncoding];
            raw = rtex_jobs_add_text(root.UTF8String, text.UTF8String, js.UTF8String);
        } else {
            raw = rtex_jobs_add_url(root.UTF8String, url.UTF8String);
        }
        NSString *err = nil;
        NSDictionary *job = rt::dict(raw, &err);
        dispatch_async(dispatch_get_main_queue(), ^{
            if (!self->_running.count) [self->_spinner stopAnimation:nil];
            if (!job) {
                [self setStatus:@"Could not add the job."];
                [self fail:@"Could not add the job" error:err];
                return;
            }
            [self reload];
            NSInteger row = [self rowForSlug:job[@"slug"]];
            if (row >= 0) [self->_list selectRowIndexes:[NSIndexSet indexSetWithIndex:(NSUInteger)row] byExtendingSelection:NO];
            [self->_tabs selectTabViewItemWithIdentifier:@"page"];
            [self setStatus:[NSString stringWithFormat:@"Added %@ — %@", job[@"company"], job[@"title"]]];
            if (tailor) [self enqueueTailor:job];
        });
    });
}

// MARK: Markdown

+ (NSAttributedString *)inline:(NSString *)text font:(NSFont *)body {
    NSAttributedStringMarkdownParsingOptions *opts = [[NSAttributedStringMarkdownParsingOptions alloc] init];
    opts.interpretedSyntax = NSAttributedStringMarkdownInterpretedSyntaxInlineOnlyPreservingWhitespace;
    NSMutableAttributedString *a = [[[NSAttributedString alloc] initWithMarkdownString:text options:opts baseURL:nil error:nil] mutableCopy]
        ?: [[NSMutableAttributedString alloc] initWithString:text];
    NSRange all = NSMakeRange(0, a.length);
    NSFont *mono = [NSFont monospacedSystemFontOfSize:body.pointSize - 1 weight:NSFontWeightRegular];
    [a addAttribute:NSForegroundColorAttributeName value:NSColor.labelColor range:all];
    [a enumerateAttribute:NSInlinePresentationIntentAttributeName inRange:all options:0 usingBlock:^(id v, NSRange r, BOOL *stop) {
        NSUInteger intent = [v unsignedIntegerValue];
        NSFont *f = body;
        if (intent & NSInlinePresentationIntentCode) f = mono;
        else if (intent & NSInlinePresentationIntentStronglyEmphasized) f = [NSFont systemFontOfSize:body.pointSize weight:NSFontWeightSemibold];
        [a addAttribute:NSFontAttributeName value:f range:r];
    }];
    return a;
}

// `| a | b |` rows (first row is the header, `|---|` rows are skipped) as an NSTextTable.
+ (NSAttributedString *)renderTable:(NSArray<NSString *> *)rows font:(NSFont *)body {
    NSMutableArray<NSArray<NSString *> *> *cells = [NSMutableArray array];
    for (NSString *row in rows) {
        NSString *r = [row stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceCharacterSet];
        if ([r hasPrefix:@"|"]) r = [r substringFromIndex:1];
        if ([r hasSuffix:@"|"]) r = [r substringToIndex:r.length - 1];
        NSArray *parts = [r componentsSeparatedByString:@"|"];
        BOOL separator = YES;
        for (NSString *p in parts) {
            NSString *t = [p stringByTrimmingCharactersInSet:[NSCharacterSet characterSetWithCharactersInString:@" -:"]];
            if (t.length) separator = NO;
        }
        if (!separator) [cells addObject:parts];
    }
    NSUInteger cols = 0;
    for (NSArray *r in cells) cols = MAX(cols, r.count);
    NSMutableAttributedString *out = [[NSMutableAttributedString alloc] init];
    if (!cols) return out;
    NSTextTable *table = [[NSTextTable alloc] init];
    table.numberOfColumns = cols;
    table.layoutAlgorithm = NSTextTableAutomaticLayoutAlgorithm;
    table.collapsesBorders = YES;
    NSFont *small = [NSFont systemFontOfSize:body.pointSize - 1];
    for (NSUInteger ri = 0; ri < cells.count; ri++) {
        for (NSUInteger ci = 0; ci < cols; ci++) {
            NSString *text = ci < cells[ri].count ? [cells[ri][ci] stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceCharacterSet] : @"";
            NSTextTableBlock *block = [[NSTextTableBlock alloc] initWithTable:table startingRow:(NSInteger)ri rowSpan:1
                                                               startingColumn:(NSInteger)ci columnSpan:1];
            [block setWidth:5 type:NSTextBlockAbsoluteValueType forLayer:NSTextBlockPadding];
            [block setWidth:0.5 type:NSTextBlockAbsoluteValueType forLayer:NSTextBlockBorder];
            [block setBorderColor:NSColor.separatorColor];
            if (ri == 0) block.backgroundColor = [NSColor.quaternaryLabelColor colorWithAlphaComponent:0.15];
            NSMutableParagraphStyle *ps = [[NSMutableParagraphStyle alloc] init];
            ps.textBlocks = @[ block ];
            NSMutableAttributedString *cell = [[self inline:text font:ri == 0 ? [NSFont systemFontOfSize:small.pointSize weight:NSFontWeightSemibold] : small] mutableCopy];
            [cell appendAttributedString:[[NSAttributedString alloc] initWithString:@"\n"]];
            [cell addAttribute:NSParagraphStyleAttributeName value:ps range:NSMakeRange(0, cell.length)];
            [out appendAttributedString:cell];
        }
    }
    return out;
}

+ (NSAttributedString *)renderMarkdown:(NSString *)md {
    NSMutableAttributedString *out = [[NSMutableAttributedString alloc] init];
    NSFont *body = [NSFont systemFontOfSize:13];
    NSFont *mono = [NSFont monospacedSystemFontOfSize:11.5 weight:NSFontWeightRegular];
    NSMutableParagraphStyle *para = [[NSMutableParagraphStyle alloc] init];
    para.paragraphSpacing = 4;
    BOOL inCode = NO;
    BOOL inFront = NO;
    NSUInteger lineNo = 0;
    NSMutableArray<NSString *> *table = [NSMutableArray array];
    NSArray<NSString *> *lines = [[md componentsSeparatedByString:@"\n"] arrayByAddingObject:@""];
    for (NSString *line in lines) {
        lineNo++;
        // Collect a Markdown table, then render it as a real (wrapping) table.
        if (!inCode && [line hasPrefix:@"|"]) {
            [table addObject:line];
            continue;
        }
        if (table.count) {
            [out appendAttributedString:[self renderTable:table font:body]];
            [table removeAllObjects];
        }
        if (lineNo == 1 && [line isEqual:@"---"]) { inFront = YES; continue; }
        if (inFront) {
            if ([line isEqual:@"---"]) inFront = NO;
            continue;
        }
        NSDictionary *attrs;
        NSString *text = line;
        if ([line hasPrefix:@"```"]) {
            inCode = !inCode;
            continue;
        }
        if (inCode) {
            attrs = @{NSFontAttributeName : mono, NSForegroundColorAttributeName : NSColor.labelColor,
                      NSBackgroundColorAttributeName : [NSColor.quaternaryLabelColor colorWithAlphaComponent:0.12]};
            [out appendAttributedString:[[NSAttributedString alloc] initWithString:[text stringByAppendingString:@"\n"] attributes:attrs]];
            continue;
        }
        NSUInteger hashes = 0;
        while (hashes < line.length && [line characterAtIndex:hashes] == '#') hashes++;
        if (hashes && hashes < line.length && [line characterAtIndex:hashes] == ' ') {
            CGFloat size = hashes == 1 ? 20 : hashes == 2 ? 16 : hashes == 3 ? 14 : 13;
            NSMutableParagraphStyle *h = [para mutableCopy];
            h.paragraphSpacingBefore = hashes <= 2 ? 10 : 6;
            attrs = @{NSFontAttributeName : [NSFont systemFontOfSize:size weight:NSFontWeightSemibold],
                      NSForegroundColorAttributeName : NSColor.labelColor, NSParagraphStyleAttributeName : h};
            text = [line substringFromIndex:hashes + 1];
            [out appendAttributedString:[[NSAttributedString alloc] initWithString:[text stringByAppendingString:@"\n"] attributes:attrs]];
            continue;
        }
        NSColor *color = NSColor.labelColor;
        if ([line containsString:@"Check this claim"] || [line containsString:@"Not applied"] ||
            ([line hasPrefix:@"- Line rule:"] && ![line hasSuffix:@"ok"])) {
            color = NSColor.systemOrangeColor;
        }
        NSAttributedStringMarkdownParsingOptions *opts = [[NSAttributedStringMarkdownParsingOptions alloc] init];
        opts.interpretedSyntax = NSAttributedStringMarkdownInterpretedSyntaxInlineOnlyPreservingWhitespace;
        NSMutableAttributedString *a = [[[NSAttributedString alloc] initWithMarkdownString:line options:opts baseURL:nil error:nil] mutableCopy]
            ?: [[NSMutableAttributedString alloc] initWithString:line];
        NSRange all = NSMakeRange(0, a.length);
        [a addAttribute:NSForegroundColorAttributeName value:color range:all];
        [a addAttribute:NSParagraphStyleAttributeName value:para range:all];
        // Inline markdown gives presentation intents; map them onto fonts.
        [a enumerateAttribute:NSInlinePresentationIntentAttributeName inRange:all options:0 usingBlock:^(id v, NSRange r, BOOL *stop) {
            NSUInteger intent = [v unsignedIntegerValue];
            NSFont *f = body;
            if (intent & NSInlinePresentationIntentCode) f = mono;
            else if (intent & NSInlinePresentationIntentStronglyEmphasized) f = [NSFont systemFontOfSize:13 weight:NSFontWeightSemibold];
            else if (intent & NSInlinePresentationIntentEmphasized) f = [NSFontManager.sharedFontManager convertFont:body toHaveTrait:NSItalicFontMask];
            [a addAttribute:NSFontAttributeName value:f range:r];
        }];
        [a appendAttributedString:[[NSAttributedString alloc] initWithString:@"\n"]];
        [out appendAttributedString:a];
    }
    return out;
}

@end
