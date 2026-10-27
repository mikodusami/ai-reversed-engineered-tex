#import "RTCore.h"

namespace rt {

id json(char *owned) {
    OwnedString s(owned);
    if (!s.p) return nil;
    NSData *data = [NSData dataWithBytesNoCopy:s.p length:strlen(s.p) freeWhenDone:NO];
    return [NSJSONSerialization JSONObjectWithData:data options:0 error:nil];
}

NSDictionary *dict(char *owned, NSString **error) {
    id obj = json(owned);
    if (![obj isKindOfClass:NSDictionary.class]) {
        if (error) *error = @"invalid response from rtex";
        return nil;
    }
    NSDictionary *d = obj;
    if (d[@"error"] && d.count == 1) {
        if (error) *error = d[@"error"];
        return nil;
    }
    return d;
}

std::vector<Span> highlight(NSString *text) {
    std::vector<Span> out;
    const char *utf8 = text.UTF8String;
    if (!utf8) return out;
    size_t count = 0;
    rtex_span *spans = rtex_highlight(reinterpret_cast<const uint8_t *>(utf8), strlen(utf8), &count);
    out.reserve(count);
    for (size_t i = 0; i < count; i++) {
        out.push_back({NSMakeRange(spans[i].start, spans[i].len), spans[i].kind});
    }
    rtex_spans_free(spans, count);
    return out;
}

}  // namespace rt
