// RTeX application entry point and menus (built in code; no nib).
#import <Cocoa/Cocoa.h>
#import "RTDocument.h"
#import "RTJobs.h"

@interface RTAppDelegate : NSObject <NSApplicationDelegate, NSMenuItemValidation>
@end

@implementation RTAppDelegate

- (void)applicationWillFinishLaunching:(NSNotification *)n {
    [NSUserDefaults.standardUserDefaults registerDefaults:@{
        RTDefaultsFastPreamble : @NO,
        RTDefaultsSinglePass : @NO,
        RTDefaultsTypesetOnSave : @NO,
        RTDefaultsShellEscape : @NO,
        RTDefaultsAutoPair : @YES,
        RTDefaultsAfterBuild : @"keep",
        RTDefaultsJobsAnalysis : @NO,
        // Saving, as TeXShop's FactoryDefaults: AutoSaveEnabled YES, KeepBackup NO,
        // WatchServer YES, EncodingMenuInSaveDialog NO, AutomaticUTF8MACtoUTF8Conversion NO.
        // SaveRelated is NO in TeXShop; RTeX turns it on (stale chapters give stale PDFs).
        RTDefaultsAutoSave : @YES,
        RTDefaultsKeepBackup : @NO,
        RTDefaultsWatchFiles : @YES,
        RTDefaultsSaveRelated : @YES,
        RTDefaultsEncodingInSavePanel : @NO,
        RTDefaultsPrecomposeUnicode : @NO,
    }];
    NSApp.mainMenu = [self buildMainMenu];
}

- (BOOL)applicationShouldOpenUntitledFile:(NSApplication *)sender {
    return ![NSUserDefaults.standardUserDefaults boolForKey:@"RTShowJobsOnLaunch"];
}

- (void)applicationDidFinishLaunching:(NSNotification *)n {
    if ([NSUserDefaults.standardUserDefaults boolForKey:@"RTShowJobsOnLaunch"]) [RTJobsController.shared showJobs:nil];
}

static NSMenuItem *Item(NSString *title, SEL action, NSString *key, NSEventModifierFlags mods = NSEventModifierFlagCommand) {
    NSMenuItem *i = [[NSMenuItem alloc] initWithTitle:title action:action keyEquivalent:key ?: @""];
    if (key.length) i.keyEquivalentModifierMask = mods;
    return i;
}

static NSMenuItem *Submenu(NSMenu *menu) {
    NSMenuItem *i = [[NSMenuItem alloc] initWithTitle:menu.title action:nil keyEquivalent:@""];
    i.submenu = menu;
    return i;
}

- (NSMenu *)buildMainMenu {
    NSMenu *main = [[NSMenu alloc] initWithTitle:@"Main"];
    NSString *app = @"RTeX";

    NSMenu *appMenu = [[NSMenu alloc] initWithTitle:app];
    [appMenu addItem:Item([@"About " stringByAppendingString:app], @selector(orderFrontStandardAboutPanel:), nil)];
    [appMenu addItem:NSMenuItem.separatorItem];
    NSMenuItem *services = Item(@"Services", nil, nil);
    services.submenu = [[NSMenu alloc] initWithTitle:@"Services"];
    NSApp.servicesMenu = services.submenu;
    [appMenu addItem:services];
    [appMenu addItem:NSMenuItem.separatorItem];
    [appMenu addItem:Item([@"Hide " stringByAppendingString:app], @selector(hide:), @"h")];
    [appMenu addItem:Item(@"Hide Others", @selector(hideOtherApplications:), @"h",
                          NSEventModifierFlagCommand | NSEventModifierFlagOption)];
    [appMenu addItem:Item(@"Show All", @selector(unhideAllApplications:), nil)];
    [appMenu addItem:NSMenuItem.separatorItem];
    [appMenu addItem:Item([@"Quit " stringByAppendingString:app], @selector(terminate:), @"q")];
    [main addItem:Submenu(appMenu)];

    NSMenu *file = [[NSMenu alloc] initWithTitle:@"File"];
    [file addItem:Item(@"New", @selector(newDocument:), @"n")];
    [file addItem:Item(@"Open…", @selector(openDocument:), @"o")];
    NSMenu *recent = [[NSMenu alloc] initWithTitle:@"Open Recent"];
    // NSDocumentController fills the submenu holding clearRecentDocuments:.
    [recent addItem:Item(@"Clear Menu", @selector(clearRecentDocuments:), nil)];
    [file addItem:Submenu(recent)];
    [file addItem:NSMenuItem.separatorItem];
    [file addItem:Item(@"Close", @selector(performClose:), @"w")];
    [file addItem:Item(@"Save", @selector(saveDocument:), @"s")];
    [file addItem:Item(@"Duplicate", @selector(duplicateDocument:), @"S")];
    [file addItem:Item(@"Save As…", @selector(saveDocumentAs:), @"S", NSEventModifierFlagCommand | NSEventModifierFlagShift | NSEventModifierFlagOption)];
    [file addItem:Item(@"Rename…", @selector(renameDocument:), nil)];
    [file addItem:Item(@"Move To…", @selector(moveDocument:), nil)];
    [file addItem:Item(@"Revert To Saved", @selector(revertDocumentToSaved:), nil)];
    NSMenu *saving = [[NSMenu alloc] initWithTitle:@"Save Options"];
    NSArray *opts = @[
        @[ @"Autosave (applies after relaunch)", RTDefaultsAutoSave ],
        @[ @"Keep Backup File (name~.tex)", RTDefaultsKeepBackup ],
        @[ @"Reload Files Changed by Other Apps", RTDefaultsWatchFiles ],
        @[ @"Save Project Files Before Typesetting", RTDefaultsSaveRelated ],
        @[ @"Encoding Menu in Save Panel", RTDefaultsEncodingInSavePanel ],
        @[ @"Precompose Unicode When Saving", RTDefaultsPrecomposeUnicode ],
    ];
    for (NSArray *o in opts) {
        NSMenuItem *mi = Item(o[0], @selector(toggleDefault:), nil);
        mi.representedObject = o[1];
        [saving addItem:mi];
    }
    [file addItem:Submenu(saving)];
    [file addItem:NSMenuItem.separatorItem];
    [file addItem:Item(@"Print…", @selector(printDocument:), @"p")];
    [main addItem:Submenu(file)];

    NSMenu *edit = [[NSMenu alloc] initWithTitle:@"Edit"];
    [edit addItem:Item(@"Undo", @selector(undo:), @"z")];
    [edit addItem:Item(@"Redo", @selector(redo:), @"Z")];
    [edit addItem:NSMenuItem.separatorItem];
    [edit addItem:Item(@"Cut", @selector(cut:), @"x")];
    [edit addItem:Item(@"Copy", @selector(copy:), @"c")];
    [edit addItem:Item(@"Paste", @selector(paste:), @"v")];
    [edit addItem:Item(@"Select All", @selector(selectAll:), @"a")];
    [edit addItem:NSMenuItem.separatorItem];
    NSMenu *find = [[NSMenu alloc] initWithTitle:@"Find"];
    NSMenuItem *f;
    f = Item(@"Find…", @selector(performTextFinderAction:), @"f"); f.tag = NSTextFinderActionShowFindInterface; [find addItem:f];
    f = Item(@"Find and Replace…", @selector(performTextFinderAction:), @"f", NSEventModifierFlagCommand | NSEventModifierFlagOption);
    f.tag = NSTextFinderActionShowReplaceInterface; [find addItem:f];
    f = Item(@"Find Next", @selector(performTextFinderAction:), @"g"); f.tag = NSTextFinderActionNextMatch; [find addItem:f];
    f = Item(@"Find Previous", @selector(performTextFinderAction:), @"G"); f.tag = NSTextFinderActionPreviousMatch; [find addItem:f];
    f = Item(@"Use Selection for Find", @selector(performTextFinderAction:), @"e"); f.tag = NSTextFinderActionSetSearchString; [find addItem:f];
    [edit addItem:Submenu(find)];
    [edit addItem:NSMenuItem.separatorItem];
    [edit addItem:Item(@"Comment / Uncomment", @selector(toggleComment:), @"/")];
    [edit addItem:Item(@"Shift Right", @selector(shiftRight:), @"]")];
    [edit addItem:Item(@"Shift Left", @selector(shiftLeft:), @"[")];
    [edit addItem:NSMenuItem.separatorItem];
    NSMenu *cc = [[NSMenu alloc] initWithTitle:@"Command Completion"];
    [cc addItem:Item(@"Complete Word (Esc)", @selector(complete:), nil)];
    [cc addItem:NSMenuItem.separatorItem];
    NSEventModifierFlags cmdCtl = NSEventModifierFlagCommand | NSEventModifierFlagControl;
    NSEventModifierFlags cmdCtlOpt = cmdCtl | NSEventModifierFlagOption;
    [cc addItem:Item(@"Next Mark", @selector(nextMark:), @"f", cmdCtl)];
    [cc addItem:Item(@"Previous Mark", @selector(previousMark:), @"g", cmdCtl)];
    [cc addItem:Item(@"Next Mark (Delete)", @selector(nextMarkDelete:), @"f", cmdCtlOpt)];
    [cc addItem:Item(@"Previous Mark (Delete)", @selector(previousMarkDelete:), @"g", cmdCtlOpt)];
    [cc addItem:Item(@"Insert Mark", @selector(insertMark:), @"8")];
    [cc addItem:Item(@"Insert Comment", @selector(insertMarkComment:), @"8", cmdCtl)];
    [edit addItem:Submenu(cc)];
    NSMenuItem *ap = Item(@"Auto-Pair Brackets", @selector(toggleDefault:), nil);
    ap.representedObject = RTDefaultsAutoPair;
    [edit addItem:ap];
    [main addItem:Submenu(edit)];

    NSMenu *typeset = [[NSMenu alloc] initWithTitle:@"Typeset"];
    [typeset addItem:Item(@"Typeset", @selector(typeset:), @"t")];
    [typeset addItem:Item(@"Stop", @selector(stopTypesetting:), @".")];
    [typeset addItem:NSMenuItem.separatorItem];
    [typeset addItem:Item(@"Show Line in PDF", @selector(syncToPDF:), @"j")];
    [typeset addItem:Item(@"Go to Next Issue", @selector(goToNextError:), @"'")];
    [typeset addItem:NSMenuItem.separatorItem];
    [typeset addItem:Item(@"Build Options…", @selector(showBuildPanel:), @"b", NSEventModifierFlagCommand | NSEventModifierFlagShift)];
    NSMenu *after = [[NSMenu alloc] initWithTitle:@"After Typesetting"];
    for (NSArray *c in @[ @[ @"Keep Build Files", @"keep" ], @[ @"Hide Build Files (only .tex and .pdf visible)", @"hide" ],
                          @[ @"Delete Build Files", @"delete" ] ]) {
        NSMenuItem *mi = Item(c[0], @selector(setAfterBuild:), nil);
        mi.representedObject = c[1];
        [after addItem:mi];
    }
    [typeset addItem:Submenu(after)];
    [typeset addItem:Item(@"Trash Build Files Now (⌥: whole folder)", @selector(trashAuxFiles:), nil)];
    [typeset addItem:NSMenuItem.separatorItem];
    NSMenuItem *t;
    t = Item(@"Precompiled Preamble (pdflatex)", @selector(toggleDefault:), nil); t.representedObject = RTDefaultsFastPreamble; [typeset addItem:t];
    t = Item(@"Single Pass (no BibTeX/reruns)", @selector(toggleDefault:), nil); t.representedObject = RTDefaultsSinglePass; [typeset addItem:t];
    t = Item(@"Typeset on Save", @selector(toggleDefault:), nil); t.representedObject = RTDefaultsTypesetOnSave; [typeset addItem:t];
    t = Item(@"Allow Shell Escape", @selector(toggleDefault:), nil); t.representedObject = RTDefaultsShellEscape; [typeset addItem:t];
    [main addItem:Submenu(typeset)];

    NSMenu *jobs = [[NSMenu alloc] initWithTitle:@"Jobs"];
    RTJobsController *jc = RTJobsController.shared;
    NSMenuItem *ji;
    ji = Item(@"Show Jobs", @selector(showJobs:), @"j", NSEventModifierFlagCommand | NSEventModifierFlagShift); ji.target = jc; [jobs addItem:ji];
    ji = Item(@"New Job…", @selector(newJob:), @"n", NSEventModifierFlagCommand | NSEventModifierFlagShift); ji.target = jc; [jobs addItem:ji];
    [jobs addItem:NSMenuItem.separatorItem];
    ji = Item(@"Write Role Analysis (slower)", @selector(toggleDefault:), nil); ji.representedObject = RTDefaultsJobsAnalysis; [jobs addItem:ji];
    ji = Item(@"Choose Job Folder…", @selector(chooseJobsFolder:), nil); ji.target = jc; [jobs addItem:ji];
    [main addItem:Submenu(jobs)];

    NSMenu *view = [[NSMenu alloc] initWithTitle:@"View"];
    [view addItem:Item(@"Zoom In", @selector(zoomIn:), @"+")];
    [view addItem:Item(@"Zoom Out", @selector(zoomOut:), @"-")];
    [view addItem:NSMenuItem.separatorItem];
    [view addItem:Item(@"Show Toolbar", @selector(toggleToolbarShown:), nil)];
    [view addItem:Item(@"Enter Full Screen", @selector(toggleFullScreen:), nil)];
    [main addItem:Submenu(view)];

    NSMenu *window = [[NSMenu alloc] initWithTitle:@"Window"];
    [window addItem:Item(@"Minimize", @selector(performMiniaturize:), @"m")];
    [window addItem:Item(@"Zoom", @selector(performZoom:), nil)];
    [window addItem:NSMenuItem.separatorItem];
    [window addItem:Item(@"Bring All to Front", @selector(arrangeInFront:), nil)];
    NSApp.windowsMenu = window;
    [main addItem:Submenu(window)];
    return main;
}

- (void)toggleDefault:(NSMenuItem *)sender {
    NSUserDefaults *ud = NSUserDefaults.standardUserDefaults;
    NSString *key = sender.representedObject;
    [ud setBool:![ud boolForKey:key] forKey:key];
}

- (BOOL)validateMenuItem:(NSMenuItem *)item {
    if (item.action == @selector(toggleDefault:)) {
        item.state = [NSUserDefaults.standardUserDefaults boolForKey:item.representedObject] ? NSControlStateValueOn
                                                                                               : NSControlStateValueOff;
    }
    return YES;
}

@end

int main(int argc, const char *argv[]) {
    @autoreleasepool {
        NSApplication *app = NSApplication.sharedApplication;
        RTAppDelegate *delegate = [[RTAppDelegate alloc] init];
        app.delegate = delegate;
        app.activationPolicy = NSApplicationActivationPolicyRegular;
        [app run];
    }
    return 0;
}
