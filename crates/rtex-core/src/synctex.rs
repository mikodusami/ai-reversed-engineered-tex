//! Native SyncTeX reader: forward (source → PDF) and inverse (PDF → source)
//! search.
//!
//! TeXShop does SyncTeX by spawning the `synctex` command-line tool through
//! an NSTask/NSPipe (`_synctexTask`, `_synctexPipe`, `_forwardSyncTask` ...
//! in the binary). We parse the `.synctex(.gz)` file once, keep it in a
//! cache keyed by mtime, and answer every later query in memory.
//!
//! The inverse search is a port of the edit query in the reference parser
//! (`synctex_parser.c` 1.21, MIT licence, © Jérôme Laurens): the same box
//! tree, the same "visible" h-box extents, the same box-boundary records and
//! the same distance functions, so a click resolves to the same line as
//! `synctex edit`. scripts/synctex_oracle.py checks this against the CLI.
//!
//! Format notes:
//! * Coordinates are in scaled points from the page's top-left corner, with
//!   the 1in TeX origin already included. `bp = sp * unit * mag/1000 / 65781.76`.
//! * `[`/`(` open v/h boxes: `tag,line:h,v:W,H,D`; `]`/`)` close them.
//! * `h`/`v` void boxes carry `W,H,D`; `k` kerns carry `W`; `g` glue, `$` math,
//!   `x` boundaries carry only `h,v`; `r` rules carry `W,H,D`.
//! * `Input:N:path` lines may appear anywhere, even inside content.

use crate::root::normalize;
use serde::Serialize;
use std::collections::HashMap;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

const SP_PER_BP: f64 = 65781.76;
const NONE: u32 = u32::MAX;
const FAR: i64 = i32::MAX as i64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Sheet,
    VBox,
    HBox,
    VoidVBox,
    VoidHBox,
    Kern,
    Glue,
    Math,
    Rule,
    /// `x` records
    Boundary,
    /// Synthesised at both ends of every h-box (as the reference parser does).
    BoxBdry,
}

impl Kind {
    fn is_box(self) -> bool {
        matches!(self, Kind::VBox | Kind::HBox | Kind::VoidVBox | Kind::VoidHBox | Kind::Sheet)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Node {
    pub kind: Kind,
    pub tag: u32,
    pub line: u32,
    pub h: i32,
    pub v: i32,
    pub w: i32,
    pub ht: i32,
    pub dp: i32,
    /// Visible extents (h-boxes grow to cover their content).
    hv: i32,
    vv: i32,
    wv: i32,
    htv: i32,
    dpv: i32,
    pub parent: u32,
    first_child: u32,
    last_child: u32,
    next: u32,
}

impl Node {
    #[allow(clippy::too_many_arguments)]
    fn new(kind: Kind, tag: u32, line: u32, h: i32, v: i32, w: i32, ht: i32, dp: i32) -> Node {
        Node {
            kind, tag, line, h, v, w, ht, dp,
            hv: h, vv: v, wv: w, htv: ht, dpv: dp,
            parent: NONE, first_child: NONE, last_child: NONE, next: NONE,
        }
    }
}

#[derive(Debug, Default)]
pub struct Page {
    pub number: u32,
    /// `nodes[0]` is the sheet.
    pub nodes: Vec<Node>,
    /// H-boxes, most recently closed first (the reference `next_hbox` chain).
    hboxes: Vec<u32>,
}

#[derive(Debug)]
pub struct Scanner {
    pub inputs: HashMap<u32, PathBuf>,
    pub pages: Vec<Page>,
    scale: f64,
    x_offset: f64,
    y_offset: f64,
    /// (tag, line) -> [(page index, node index)] for forward lookup.
    by_line: HashMap<(u32, u32), Vec<(u32, u32)>>,
}

/// A rectangle in PDF points, origin top-left, y growing downward
/// (the convention `synctex view` reports).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ForwardResult {
    pub page: u32,
    /// Line boxes holding the material from the requested line.
    pub rects: Vec<Rect>,
    /// The source line that matched (may differ from the request if the
    /// requested line produced no output).
    pub matched_line: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct InverseResult {
    pub file: PathBuf,
    pub line: u32,
}

// ------------------------------------------------------------------ parsing

struct PageBuilder {
    page: Page,
    stack: Vec<u32>,
    /// Leading `x` records per open box, waiting for the next non-`x` node
    /// whose tag/line they take (`x_handle` in the reference parser: the
    /// first boundaries of a box often carry a stale line number).
    pending: Vec<Vec<u32>>,
    last_k: u32,
    last_g: u32,
}

impl PageBuilder {
    fn new(number: u32) -> PageBuilder {
        let mut page = Page { number, nodes: Vec::with_capacity(1024), hboxes: Vec::new() };
        page.nodes.push(Node::new(Kind::Sheet, 0, 0, 0, 0, 0, 0, 0));
        PageBuilder { page, stack: vec![0], pending: vec![Vec::new()], last_k: NONE, last_g: NONE }
    }

    fn parent(&self) -> u32 {
        *self.stack.last().unwrap()
    }

    fn append(&mut self, mut n: Node) -> u32 {
        let parent = self.parent();
        let idx = self.page.nodes.len() as u32;
        n.parent = parent;
        let p = &mut self.page.nodes[parent as usize];
        let prev = p.last_child;
        if prev == NONE {
            p.first_child = idx;
        }
        p.last_child = idx;
        if prev != NONE {
            self.page.nodes[prev as usize].next = idx;
        }
        self.page.nodes.push(n);
        idx
    }

    /// Give the pending leading boundaries the tag/line of `model`.
    fn resolve_pending(&mut self, model: u32) {
        let (t, l) = (self.page.nodes[model as usize].tag, self.page.nodes[model as usize].line);
        if let Some(list) = self.pending.last_mut() {
            for x in list.drain(..) {
                self.page.nodes[x as usize].tag = t;
                self.page.nodes[x as usize].line = l;
            }
        }
    }

    fn open_box(&mut self, n: Node) -> u32 {
        let idx = self.append(n);
        self.stack.push(idx);
        self.pending.push(Vec::new());
        self.last_k = NONE;
        self.last_g = NONE;
        idx
    }

    fn close_box(&mut self, idx: u32) {
        self.stack.pop();
        self.pending.pop();
        self.resolve_pending(idx);
    }

    fn boundary(&mut self, n: Node) {
        let prev = self.page.nodes[self.parent() as usize].last_child;
        let leading = prev != NONE && self.page.nodes[prev as usize].kind == Kind::BoxBdry
            || self.pending.last().is_some_and(|p| !p.is_empty());
        let idx = self.append(n);
        if leading {
            self.pending.last_mut().unwrap().push(idx);
        }
        self.contain_point(n.h, n.v);
        self.last_k = NONE;
        self.last_g = NONE;
    }

    fn parent_is_hbox(&self) -> bool {
        self.page.nodes[self.parent() as usize].kind == Kind::HBox
    }

    fn contain_point(&mut self, h: i32, v: i32) {
        if self.parent_is_hbox() {
            let p = self.parent() as usize;
            make_hbox_contain(&mut self.page.nodes[p], h, h, v, v);
        }
    }

    fn contain_box(&mut self, min_h: i32, max_h: i32, min_v: i32, max_v: i32) {
        if self.parent_is_hbox() {
            let p = self.parent() as usize;
            make_hbox_contain(&mut self.page.nodes[p], min_h, max_h, min_v, max_v);
        }
    }

    fn open_hbox(&mut self, n: Node) {
        self.open_box(n);
        // Leading box boundary, inheriting the box's tag/line/position.
        self.append(Node::new(Kind::BoxBdry, n.tag, n.line, n.h, n.v, 0, 0, 0));
        self.last_k = NONE;
        self.last_g = NONE;
    }

    fn close_hbox(&mut self) {
        let idx = self.parent();
        if self.page.nodes[idx as usize].kind != Kind::HBox {
            return;
        }
        let b = self.page.nodes[idx as usize];
        let first = b.first_child;
        // Trailing box boundary: tag/line of the last child, at the visible right edge.
        let last = self.page.nodes[b.last_child as usize];
        self.append(Node::new(Kind::BoxBdry, last.tag, last.line, b.hv + b.wv, b.vv, 0, 0, 0));
        // The leading boundary takes the visible origin and the first real child's line.
        let second = self.page.nodes[first as usize].next;
        let second_line = (second != NONE).then(|| self.page.nodes[second as usize].line);
        {
            let f = &mut self.page.nodes[first as usize];
            f.h = b.hv;
            f.v = b.vv;
            if let Some(l) = second_line {
                f.line = l;
            }
        }
        // A trailing kern+glue pair (end of paragraph) is attributed to the
        // material before it.
        if self.last_k != NONE && self.last_g != NONE {
            let mut c = self.page.nodes[idx as usize].first_child;
            while c != NONE {
                let nx = self.page.nodes[c as usize].next;
                if nx == self.last_k {
                    let (t, l) = (self.page.nodes[c as usize].tag, self.page.nodes[c as usize].line);
                    for k in [self.last_k, self.last_g] {
                        self.page.nodes[k as usize].tag = t;
                        self.page.nodes[k as usize].line = l;
                    }
                    break;
                }
                c = nx;
            }
        }
        self.close_box(idx);
        self.page.hboxes.push(idx);
        let b = self.page.nodes[idx as usize];
        let (min_h, max_h) = ordered(b.hv, b.hv + b.wv);
        self.contain_box(min_h, max_h, b.vv - b.htv, b.vv + b.dpv);
        self.last_k = NONE;
        self.last_g = NONE;
    }

    fn finish(mut self) -> Page {
        self.page.hboxes.reverse();
        self.page
    }
}

fn ordered(a: i32, b: i32) -> (i32, i32) {
    if a <= b { (a, b) } else { (b, a) }
}

/// `_synctex_make_hbox_contain_box`: grow the visible extents.
fn make_hbox_contain(n: &mut Node, min_h: i32, max_h: i32, min_v: i32, max_v: i32) {
    if n.wv < 0 {
        let max = n.hv;
        let min = max + n.wv;
        if min_h < min {
            n.wv = min_h - max;
        } else if max_h > max {
            n.hv = max_h;
            n.wv = min - max_h;
        }
    } else {
        let min = n.hv;
        let max = min + n.wv;
        if min_h < min {
            n.hv = min_h;
            n.wv = max - min_h;
        } else if max_h > max {
            n.wv = max_h - min;
        }
    }
    let c = n.vv;
    if min_v < c - n.htv {
        n.htv = c - min_v;
    } else if max_v > c + n.dpv {
        n.dpv = max_v - c;
    }
}

impl Scanner {
    /// Load `foo.synctex.gz` or `foo.synctex`; `pdf` may also be passed.
    pub fn open(path: &Path) -> io::Result<Scanner> {
        let file = locate(path).ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, format!("no SyncTeX file for {}", path.display()))
        })?;
        let raw = std::fs::read(&file)?;
        let text = if raw.starts_with(&[0x1f, 0x8b]) {
            let mut out = Vec::with_capacity(raw.len() * 6);
            flate2::read::MultiGzDecoder::new(&raw[..]).read_to_end(&mut out)?;
            out
        } else {
            raw
        };
        Ok(Self::parse(&text))
    }

    pub fn parse(data: &[u8]) -> Scanner {
        let mut inputs = HashMap::new();
        let (mut unit, mut mag, mut xo, mut yo) = (1.0f64, 1000.0f64, 0.0f64, 0.0f64);
        let mut pages: Vec<Page> = Vec::new();
        let mut in_content = false;
        let mut cur: Option<PageBuilder> = None;
        let mut form_depth = 0u32;
        let (mut last_h, mut last_v) = (0i32, 0i32);

        for raw_line in data.split(|&b| b == b'\n') {
            let line = raw_line.strip_suffix(b"\r").unwrap_or(raw_line);
            let Some(&c) = line.first() else { continue };
            if c == b'I' && line.starts_with(b"Input:") {
                let rest = &line[6..];
                if let Some(p) = rest.iter().position(|&b| b == b':') {
                    if let Some(tag) = parse_u32(&rest[..p]) {
                        let path = String::from_utf8_lossy(&rest[p + 1..]).into_owned();
                        inputs.insert(tag, normalize(Path::new(&path)));
                    }
                }
                continue;
            }
            if !in_content {
                let s = String::from_utf8_lossy(line);
                if let Some(v) = s.strip_prefix("Unit:") {
                    unit = v.trim().parse().unwrap_or(1.0);
                } else if let Some(v) = s.strip_prefix("Magnification:") {
                    mag = v.trim().parse().unwrap_or(1000.0);
                } else if let Some(v) = s.strip_prefix("X Offset:") {
                    xo = v.trim().parse().unwrap_or(0.0);
                } else if let Some(v) = s.strip_prefix("Y Offset:") {
                    yo = v.trim().parse().unwrap_or(0.0);
                } else if s.starts_with("Content:") {
                    in_content = true;
                }
                continue;
            }
            match c {
                b'{' => {
                    let number = parse_u32(&line[1..]).unwrap_or(pages.len() as u32 + 1);
                    cur = Some(PageBuilder::new(number));
                }
                b'}' => {
                    if let Some(p) = cur.take() {
                        pages.push(p.finish());
                    }
                }
                b'<' => form_depth += 1,
                b'>' => form_depth = form_depth.saturating_sub(1),
                _ if form_depth > 0 => {}
                b']' => {
                    if let Some(pb) = cur.as_mut() {
                        let top = pb.parent();
                        if pb.stack.len() > 1 && pb.page.nodes[top as usize].kind == Kind::VBox {
                            pb.close_box(top);
                        }
                        pb.last_k = NONE;
                        pb.last_g = NONE;
                    }
                }
                b')' => {
                    if let Some(pb) = cur.as_mut() {
                        pb.close_hbox();
                    }
                }
                b'[' | b'(' | b'h' | b'v' | b'k' | b'g' | b'$' | b'x' | b'r' => {
                    let Some(pb) = cur.as_mut() else { continue };
                    let kind = match c {
                        b'[' => Kind::VBox,
                        b'(' => Kind::HBox,
                        b'h' => Kind::VoidHBox,
                        b'v' => Kind::VoidVBox,
                        b'k' => Kind::Kern,
                        b'g' => Kind::Glue,
                        b'$' => Kind::Math,
                        b'r' => Kind::Rule,
                        _ => Kind::Boundary,
                    };
                    let Some(node) = parse_node(kind, &line[1..], last_h, last_v) else { continue };
                    last_h = node.h;
                    last_v = node.v;
                    match kind {
                        Kind::HBox => pb.open_hbox(node),
                        Kind::VBox => {
                            pb.open_box(node);
                        }
                        Kind::VoidHBox => {
                            let idx = pb.append(node);
                            pb.resolve_pending(idx);
                            let (a, b) = ordered(node.h, node.h + node.w);
                            pb.contain_box(a, b, node.v - node.ht, node.v + node.dp);
                            pb.last_k = NONE;
                            pb.last_g = NONE;
                        }
                        Kind::Kern => {
                            let idx = pb.append(node);
                            pb.resolve_pending(idx);
                            let (a, b) = ordered(node.h - node.w, node.h);
                            pb.contain_box(a, b, node.v, node.v);
                            pb.last_k = idx;
                            pb.last_g = NONE;
                        }
                        Kind::Glue => {
                            let idx = pb.append(node);
                            pb.resolve_pending(idx);
                            pb.contain_point(node.h, node.v);
                            if pb.last_k != NONE {
                                pb.last_g = idx;
                            } else {
                                pb.last_g = NONE;
                            }
                        }
                        Kind::Boundary => pb.boundary(node),
                        Kind::Math => {
                            let idx = pb.append(node);
                            pb.resolve_pending(idx);
                            pb.contain_point(node.h, node.v);
                            pb.last_k = NONE;
                            pb.last_g = NONE;
                        }
                        _ => {
                            // void vbox, rule: no growth of the parent.
                            let idx = pb.append(node);
                            pb.resolve_pending(idx);
                            pb.last_k = NONE;
                            pb.last_g = NONE;
                        }
                    }
                }
                b'P' if line.starts_with(b"Postamble:") => in_content = false,
                _ => {} // `!` byte counts, `c` characters, `f` form refs
            }
        }
        if let Some(p) = cur.take() {
            pages.push(p.finish());
        }

        let mut by_line: HashMap<(u32, u32), Vec<(u32, u32)>> = HashMap::new();
        for (pi, page) in pages.iter().enumerate() {
            for (ni, n) in page.nodes.iter().enumerate() {
                if !matches!(n.kind, Kind::Sheet | Kind::BoxBdry) {
                    by_line.entry((n.tag, n.line)).or_default().push((pi as u32, ni as u32));
                }
            }
        }
        let scale = unit * mag / 1000.0 / SP_PER_BP;
        Scanner { inputs, pages, scale, x_offset: xo * scale, y_offset: yo * scale, by_line }
    }

    fn bp_x(&self, sp: i32) -> f64 {
        sp as f64 * self.scale + self.x_offset
    }
    fn bp_y(&self, sp: i32) -> f64 {
        sp as f64 * self.scale + self.y_offset
    }

    fn rect_of(&self, n: &Node) -> Rect {
        Rect {
            x: self.bp_x(n.h),
            y: self.bp_y(n.v - n.ht),
            width: n.w as f64 * self.scale,
            height: (n.ht + n.dp) as f64 * self.scale,
        }
    }

    /// Box rectangle using the visible extents for h-boxes.
    fn visible_rect(&self, n: &Node) -> Rect {
        if n.kind != Kind::HBox {
            return self.rect_of(n);
        }
        let (min_h, max_h) = ordered(n.hv, n.hv + n.wv);
        Rect {
            x: self.bp_x(min_h),
            y: self.bp_y(n.vv - n.htv.abs()),
            width: (max_h - min_h) as f64 * self.scale,
            height: (n.htv.abs() + n.dpv.abs()) as f64 * self.scale,
        }
    }

    /// Tags whose input path refers to `file` (absolute or as TeX named it).
    fn tags_for(&self, file: &Path) -> Vec<u32> {
        let want = normalize(file);
        let mut exact: Vec<u32> = self.inputs.iter().filter(|(_, p)| **p == want).map(|(t, _)| *t).collect();
        if exact.is_empty() {
            let name = want.to_string_lossy().trim_start_matches("./").to_string();
            exact = self
                .inputs
                .iter()
                .filter(|(_, p)| p.to_string_lossy().ends_with(&format!("/{name}")))
                .map(|(t, _)| *t)
                .collect();
        }
        exact
    }

    /// Source line → PDF location. `page_hint` (1-based) is preferred when it
    /// holds a match, as with `synctex view`.
    pub fn forward(&self, file: &Path, line: u32, page_hint: Option<u32>) -> Option<ForwardResult> {
        let tags = self.tags_for(file);
        if tags.is_empty() {
            return None;
        }
        // Try the line itself, then search outward when a line produced no
        // boxes (blank lines, comments).
        for delta in 0..=40u32 {
            let candidates: &[u32] = if delta == 0 { &[line] } else { &[line + delta, line.saturating_sub(delta)] };
            for &l in candidates {
                if l == 0 {
                    continue;
                }
                let hits: Vec<(u32, u32)> =
                    tags.iter().filter_map(|t| self.by_line.get(&(*t, l))).flatten().copied().collect();
                if hits.is_empty() {
                    continue;
                }
                let page_idx = page_hint
                    .and_then(|hint| hits.iter().find(|(p, _)| self.pages[*p as usize].number == hint))
                    .or_else(|| hits.iter().min_by_key(|(p, _)| *p))
                    .map(|(p, _)| *p)?;
                let page = &self.pages[page_idx as usize];
                let mut rects: Vec<Rect> = Vec::new();
                let mut seen = Vec::new();
                for &(_, ni) in hits.iter().filter(|(p, _)| *p == page_idx) {
                    // Highlight the whole line: climb to the nearest h-box, then
                    // to the outermost h-box of that chain (as `synctex view` does).
                    let mut idx = ni;
                    while !matches!(page.nodes[idx as usize].kind, Kind::HBox | Kind::VoidHBox) {
                        let p = page.nodes[idx as usize].parent;
                        if p == NONE || p == 0 {
                            break;
                        }
                        idx = p;
                    }
                    loop {
                        let p = page.nodes[idx as usize].parent;
                        if p == NONE || p == 0 || page.nodes[p as usize].kind != Kind::HBox {
                            break;
                        }
                        idx = p;
                    }
                    let node = &page.nodes[idx as usize];
                    if !node.kind.is_box() || node.kind == Kind::Sheet || seen.contains(&idx) {
                        continue;
                    }
                    seen.push(idx);
                    let r = self.visible_rect(node);
                    if r.width > 0.0 || r.height > 0.0 {
                        rects.push(r);
                    }
                }
                if rects.is_empty() {
                    continue;
                }
                rects.sort_by(|a, b| a.y.total_cmp(&b.y).then(a.x.total_cmp(&b.x)));
                rects.dedup();
                return Some(ForwardResult { page: page.number, rects, matched_line: l });
            }
        }
        None
    }

    /// PDF location (page 1-based, x/y in bp from top-left) → source line.
    pub fn inverse(&self, page_no: u32, x: f64, y: f64) -> Option<InverseResult> {
        let page = self.pages.iter().find(|p| p.number == page_no)?;
        let hit = Hit { h: ((x - self.x_offset) / self.scale) as i64, v: ((y - self.y_offset) / self.scale) as i64 };
        let q = Query { p: page };
        let idx = q.edit(hit)?;
        let n = &page.nodes[idx as usize];
        Some(InverseResult { file: self.inputs.get(&n.tag)?.clone(), line: n.line })
    }
}

// ------------------------------------------------------- edit query (port)

#[derive(Clone, Copy)]
struct Hit {
    h: i64,
    v: i64,
}

/// Node + distance (`synctex_nd_s`); `node == NONE` with `FAR` is ND_0.
#[derive(Clone, Copy)]
struct Nd {
    node: u32,
    d: i64,
}

const ND0: Nd = Nd { node: NONE, d: FAR };

struct Query<'a> {
    p: &'a Page,
}

impl Query<'_> {
    fn n(&self, i: u32) -> &Node {
        &self.p.nodes[i as usize]
    }
    fn children(&self, i: u32) -> impl Iterator<Item = u32> + '_ {
        let mut c = self.n(i).first_child;
        std::iter::from_fn(move || {
            if c == NONE {
                return None;
            }
            let r = c;
            c = self.n(c).next;
            Some(r)
        })
    }
    fn has_child(&self, i: u32) -> bool {
        self.n(i).first_child != NONE
    }
    fn parent_dims(&self, i: u32) -> (i64, i64) {
        let p = self.n(i).parent;
        if p == NONE {
            return (0, 0);
        }
        let p = self.n(p);
        (p.ht.unsigned_abs() as i64, p.dp.unsigned_abs() as i64)
    }

    fn h_ordered(&self, hit: Hit, i: u32) -> i64 {
        let n = self.n(i);
        let span = |min: i64, w: i64| {
            let max = min + w.abs();
            if hit.h < min { min - hit.h } else if hit.h > max { max - hit.h } else { 0 }
        };
        match n.kind {
            Kind::VBox | Kind::VoidVBox | Kind::VoidHBox | Kind::Sheet => span(n.h as i64, n.w as i64),
            Kind::HBox => span(n.hv as i64, n.wv as i64),
            Kind::Kern => {
                let w = n.w as i64;
                let (min, max) = if w < 0 { (n.h as i64, n.h as i64 - w) } else { (n.h as i64 - w, n.h as i64) };
                let med = (min + max) / 2;
                if hit.h < min {
                    min - hit.h + 1
                } else if hit.h > max {
                    max - hit.h - 1
                } else if hit.h > med {
                    max - hit.h + 1
                } else {
                    min - hit.h - 1
                }
            }
            Kind::Rule | Kind::Glue | Kind::Math | Kind::Boundary | Kind::BoxBdry => n.h as i64 - hit.h,
        }
    }

    fn v_ordered(&self, hit: Hit, i: u32) -> i64 {
        let n = self.n(i);
        let span = |min: i64, max: i64| if hit.v < min { min - hit.v } else if hit.v > max { max - hit.v } else { 0 };
        match n.kind {
            Kind::VBox | Kind::VoidVBox | Kind::VoidHBox | Kind::Sheet => {
                let v = n.v as i64;
                span(v - (n.ht as i64).abs(), v + (n.dp as i64).abs())
            }
            Kind::HBox => {
                let v = n.vv as i64;
                span(v - (n.htv as i64).abs(), v + (n.dpv as i64).abs())
            }
            Kind::Rule | Kind::Kern | Kind::Glue | Kind::Math => {
                let (ph, pd) = self.parent_dims(i);
                let v = n.v as i64;
                span(v - ph, v + pd)
            }
            Kind::Boundary | Kind::BoxBdry => FAR,
        }
    }

    fn in_box(&self, hit: Hit, i: u32) -> bool {
        self.h_ordered(hit, i) == 0 && self.v_ordered(hit, i) == 0
    }

    /// `_synctex_point_node_distance_v2`
    fn distance(&self, hit: Hit, i: u32) -> i64 {
        let n = self.n(i);
        let (h, v, w) = (n.h as i64, n.v as i64, n.w as i64);
        match n.kind {
            Kind::VBox | Kind::Sheet => dist_to_box(hit, h, h + w.abs(), v - (n.ht as i64).abs(), v + (n.dp as i64).abs()),
            Kind::HBox => {
                let (hv, vv) = (n.hv as i64, n.vv as i64);
                dist_to_box(hit, hv, hv + (n.wv as i64).abs(), vv - (n.htv as i64).abs(), vv + (n.dpv as i64).abs())
            }
            Kind::VoidVBox | Kind::VoidHBox => {
                let (t, b) = (v - (n.ht as i64).abs(), v + (n.dp as i64).abs());
                let left = dist_to_box(hit, h, h, t, b);
                let r = h + w.abs();
                left.min(dist_to_box(hit, r, r, t, b))
            }
            Kind::Kern => {
                let (ph, _) = self.parent_dims_data(i);
                let a = dist_to_box(hit, h, h, v - ph, v);
                let h2 = h - w;
                a.min(dist_to_box(hit, h2, h2, v - ph, v))
            }
            Kind::Glue | Kind::Math | Kind::Boundary | Kind::BoxBdry => {
                let (ph, _) = self.parent_dims_data(i);
                dist_to_box(hit, h, h, v - ph, v)
            }
            Kind::Rule => FAR,
        }
    }

    fn parent_dims_data(&self, i: u32) -> (i64, i64) {
        self.parent_dims(i)
    }

    fn same_tag_prefer(&self, cur: Nd, cand: u32) -> bool {
        let (a, b) = (self.n(cur.node), self.n(cand));
        a.tag == b.tag && a.line > b.line
    }

    /// `_synctex_eq_deepest_container_v2`
    fn deepest_v2(&self, hit: Hit, i: u32) -> Option<u32> {
        if !self.has_child(i) {
            return None;
        }
        for c in self.children(i) {
            if self.in_box(hit, c) {
                if let Some(d) = self.deepest_v2(hit, c) {
                    return Some(d);
                }
            }
        }
        if matches!(self.n(i).kind, Kind::VBox | Kind::Sheet) {
            let mut best = ND0;
            for c in self.children(i) {
                if self.has_child(c) {
                    let d = self.distance(hit, c);
                    if d <= best.d {
                        best = Nd { node: c, d };
                    }
                }
            }
            if best.node != NONE {
                return Some(best.node);
            }
        }
        self.in_box(hit, i).then_some(i)
    }

    /// `_synctex_eq_deepest_container_v3`
    fn deepest_v3(&self, hit: Hit, i: u32) -> Nd {
        if !self.has_child(i) {
            return ND0;
        }
        for c in self.children(i) {
            let d = self.deepest_v3(hit, c);
            if d.node != NONE {
                return d;
            }
        }
        if matches!(self.n(i).kind, Kind::VBox | Kind::Sheet) {
            let mut best = ND0;
            for c in self.children(i) {
                if self.has_child(c) {
                    let d = self.distance(hit, c);
                    if d < best.d {
                        best = Nd { node: c, d };
                    }
                }
            }
            if best.node != NONE {
                return best;
            }
        }
        if self.in_box(hit, i) {
            Nd { node: i, d: 0 }
        } else {
            ND0
        }
    }

    /// `__synctex_closest_deep_child_v2`
    fn closest_deep_child(&self, hit: Hit, i: u32) -> Nd {
        let mut best = ND0;
        for c in self.children(i) {
            let nd = if self.n(c).kind.is_box() {
                self.closest_deep_child(hit, c)
            } else {
                Nd { node: c, d: self.distance(hit, c) }
            };
            let not_kern = nd.node == NONE || self.n(nd.node).kind != Kind::Kern;
            if nd.d < best.d || (nd.d == best.d && not_kern) {
                best = nd;
            }
        }
        best
    }

    /// `__synctex_eq_get_closest_children_in_hbox_v2` (the v-box variant in
    /// 1.21 never visits any child, so it is not needed).
    fn closest_children_in_hbox(&self, hit: Hit, i: u32) -> (Nd, Nd) {
        let (mut l, mut r) = (ND0, ND0);
        for c in self.children(i) {
            let d = self.h_ordered(hit, c);
            if d > 0 {
                // Closer wins; on a tie, the smaller line of the same input.
                if r.d > d || (r.d == d && r.node != NONE && self.same_tag_prefer(r, c)) {
                    r = Nd { node: c, d };
                }
            } else if d == 0 {
                if self.has_child(c) {
                    return self.closest_children_in_box(hit, c);
                }
                l = Nd { node: c, d };
            } else {
                let d = -d;
                // Closer wins; on a tie, the smaller line of the same input.
                if l.d > d || (l.d == d && l.node != NONE && self.same_tag_prefer(l, c)) {
                    l = Nd { node: c, d };
                }
            }
        }
        for side in [&mut l, &mut r] {
            if side.node != NONE {
                let nd = self.deepest_v3(hit, side.node);
                if nd.node != NONE {
                    *side = nd;
                }
                let nd = self.closest_deep_child(hit, side.node);
                if nd.node != NONE {
                    side.node = nd.node;
                }
            }
        }
        (l, r)
    }

    fn closest_children_in_box(&self, hit: Hit, i: u32) -> (Nd, Nd) {
        if self.has_child(i) && self.n(i).kind == Kind::HBox {
            self.closest_children_in_hbox(hit, i)
        } else {
            (ND0, ND0)
        }
    }

    /// `smallest_container`: the h-box with the smaller visible area.
    fn smallest(&self, a: u32, b: u32) -> u32 {
        let (na, nb) = (self.n(a), self.n(b));
        let th = |n: &Node| (n.dpv as i64).abs() + (n.htv as i64).abs();
        let area = |n: &Node| th(n) * (n.wv as i64).abs();
        match area(na).cmp(&area(nb)) {
            std::cmp::Ordering::Less => return a,
            std::cmp::Ordering::Greater => return b,
            _ => {}
        }
        if (na.w as i64).abs() > (nb.w as i64).abs() {
            return a;
        }
        if (na.w as i64).abs() < (nb.w as i64).abs() {
            return b;
        }
        if th(na) <= th(nb) { a } else { b }
    }

    /// `synctex_iterator_new_edit`: returns the first result.
    fn edit(&self, hit: Hit) -> Option<u32> {
        let mut container = None;
        for (k, &hb) in self.p.hboxes.iter().enumerate() {
            if self.in_box(hit, hb) {
                let mut node = hb;
                for &next in &self.p.hboxes[k + 1..] {
                    if self.in_box(hit, next) {
                        node = self.smallest(next, node);
                    }
                }
                container = Some(node);
                break;
            }
        }
        let (l, r, fallback) = match container {
            Some(node) => {
                let node = self.deepest_v2(hit, node).unwrap_or(node);
                let (l, r) = self.closest_children_in_box(hit, node);
                (l, r, node)
            }
            None => {
                let first = self.n(0).first_child;
                if first == NONE {
                    return None;
                }
                (self.closest_deep_child(hit, first), ND0, first)
            }
        };
        let pick = if l.node != NONE && r.node != NONE {
            let (nl, nr) = (self.n(l.node), self.n(r.node));
            if nl.tag != nr.tag || nl.line != nr.line {
                if nr.line < nl.line || (nr.line == nl.line && l.d > r.d) {
                    r.node
                } else {
                    l.node
                }
            } else if l.d > r.d {
                r.node
            } else {
                l.node
            }
        } else if r.node != NONE {
            r.node
        } else if l.node != NONE {
            l.node
        } else {
            fallback
        };
        Some(pick)
    }
}

/// `_synctex_distance_to_box_v2`: the nine-region distance.
fn dist_to_box(hit: Hit, min_h: i64, max_h: i64, min_v: i64, max_v: i64) -> i64 {
    let dh = if hit.h < min_h { min_h - hit.h } else if hit.h > max_h { hit.h - max_h } else { 0 };
    let dv = if hit.v < min_v { min_v - hit.v } else if hit.v > max_v { hit.v - max_v } else { 0 };
    dh + dv
}

/// `tag,line[,col]:h,v[:W[,H,D]]`
fn parse_node(kind: Kind, s: &[u8], last_h: i32, last_v: i32) -> Option<Node> {
    let mut it = s.split(|&b| b == b':');
    let link = it.next()?;
    let pos = it.next()?;
    let dims = it.next();
    let mut l = link.split(|&b| b == b',');
    let tag = parse_u32(l.next()?)?;
    let line = parse_u32(l.next()?)?;
    let mut p = pos.split(|&b| b == b',');
    let h = parse_coord(p.next()?, last_h)?;
    let v = parse_coord(p.next()?, last_v)?;
    let (mut w, mut ht, mut dp) = (0, 0, 0);
    if let Some(d) = dims {
        let mut d = d.split(|&b| b == b',');
        w = d.next().and_then(parse_i32).unwrap_or(0);
        ht = d.next().and_then(parse_i32).unwrap_or(0);
        dp = d.next().and_then(parse_i32).unwrap_or(0);
    }
    Some(Node::new(kind, tag, line, h, v, w, ht, dp))
}

fn parse_coord(s: &[u8], last: i32) -> Option<i32> {
    if s == b"=" { Some(last) } else { parse_i32(s) }
}

fn parse_u32(s: &[u8]) -> Option<u32> {
    if s.is_empty() {
        return None;
    }
    let mut v: u32 = 0;
    for &b in s {
        if !b.is_ascii_digit() {
            return None;
        }
        v = v.checked_mul(10)?.checked_add((b - b'0') as u32)?;
    }
    Some(v)
}

fn parse_i32(s: &[u8]) -> Option<i32> {
    match s.first()? {
        b'-' => parse_u32(&s[1..]).map(|v| -(v as i64) as i32),
        _ => parse_u32(s).map(|v| v as i32),
    }
}

/// Find the SyncTeX file belonging to a `.pdf`, `.tex`, or `.synctex(.gz)` path.
pub fn locate(path: &Path) -> Option<PathBuf> {
    let s = path.to_string_lossy();
    if s.ends_with(".synctex.gz") || s.ends_with(".synctex") {
        return path.exists().then(|| path.to_path_buf());
    }
    let stem = path.with_extension("");
    for ext in ["synctex.gz", "synctex"] {
        let cand = PathBuf::from(format!("{}.{ext}", stem.display()));
        if cand.exists() {
            return Some(cand);
        }
    }
    None
}

type Cache = Mutex<HashMap<PathBuf, (SystemTime, Arc<Scanner>)>>;

/// Shared scanner, re-parsed only when the SyncTeX file changes on disk.
pub fn cached(path: &Path) -> io::Result<Arc<Scanner>> {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let file = locate(path)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("no SyncTeX file for {}", path.display())))?;
    let mtime = std::fs::metadata(&file)?.modified()?;
    let cache = CACHE.get_or_init(Default::default);
    if let Some((t, s)) = cache.lock().unwrap().get(&file) {
        if *t == mtime {
            return Ok(s.clone());
        }
    }
    let scanner = Arc::new(Scanner::open(&file)?);
    cache.lock().unwrap().insert(file, (mtime, scanner.clone()));
    Ok(scanner)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "SyncTeX Version:1\nInput:1:/doc/./a.tex\nOutput:pdf\nMagnification:1000\nUnit:1\nX Offset:0\nY Offset:0\nContent:\n!100\n{1\n[1,3:4736286,46220574:26673152,41484288,0\n(1,5:8799518,11617568:22609920,773689,0\nx1,5:9000000,11617568\ng1,6:12000000,11617568\nx1,6:13000000,11617568\n)\n(1,7:8799518,13518113:22609920,642250,275254\nx1,7:9000000,13518113\n)\n]\n}1\nPostamble:\nCount:5\n";

    #[test]
    fn parses_and_forward() {
        let s = Scanner::parse(SAMPLE.as_bytes());
        assert_eq!(s.pages.len(), 1);
        assert_eq!(s.inputs[&1], PathBuf::from("/doc/a.tex"));
        let r = s.forward(Path::new("/doc/a.tex"), 7, None).unwrap();
        assert_eq!(r.page, 1);
        assert_eq!(r.rects.len(), 1);
        assert!((r.rects[0].x - 133.768).abs() < 0.01);
        assert!(s.forward(Path::new("/doc/a.tex"), 4, None).is_some());
    }

    #[test]
    fn inverse_picks_leaf_left_of_point() {
        let s = Scanner::parse(SAMPLE.as_bytes());
        let y = 11_617_000.0 / SP_PER_BP;
        assert_eq!(s.inverse(1, 12_500_000.0 / SP_PER_BP, y).unwrap().line, 6);
        // The leading `x1,5` of the box takes the line of the glue after it
        // (reference-parser behaviour), so this point also maps to line 6.
        assert_eq!(s.inverse(1, 9_100_000.0 / SP_PER_BP, y).unwrap().line, 6);
        // The second box has no later node, so its leading x keeps line 7.
        assert_eq!(s.inverse(1, 9_100_000.0 / SP_PER_BP, 13_500_000.0 / SP_PER_BP).unwrap().line, 7);
    }

    #[test]
    fn box_boundaries_are_added() {
        let s = Scanner::parse(SAMPLE.as_bytes());
        let p = &s.pages[0];
        let bdry = p.nodes.iter().filter(|n| n.kind == Kind::BoxBdry).count();
        assert_eq!(bdry, 4);
        assert_eq!(p.hboxes.len(), 2);
    }
}
