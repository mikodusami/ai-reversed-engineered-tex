// Thin C++/Objective-C bridge over the rtex C ABI.
#pragma once
#import <Foundation/Foundation.h>
#include <string>
#include <vector>
#include "rtex.h"

namespace rt {

// Owns a char* from rtex and frees it on scope exit.
struct OwnedString {
    char *p = nullptr;
    explicit OwnedString(char *s) : p(s) {}
    ~OwnedString() { rtex_string_free(p); }
    OwnedString(const OwnedString &) = delete;
    OwnedString &operator=(const OwnedString &) = delete;
};

// Parse a JSON result from rtex into Foundation objects (nil on failure).
id json(char *owned);

// A dictionary result, or nil when rtex returned {"error": ...}.
NSDictionary *dict(char *owned, NSString **error = nullptr);

inline const char *c(NSString *s) { return s ? s.UTF8String : ""; }

struct Span {
    NSRange range;
    uint8_t kind;
};

// Highlight `text` (offsets relative to its start, UTF-16).
std::vector<Span> highlight(NSString *text);

}  // namespace rt
