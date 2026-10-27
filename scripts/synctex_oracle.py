#!/usr/bin/env python3
"""Compare rtex's native SyncTeX against the reference `synctex` CLI.

usage: synctex_oracle.py <rtex binary> <pdf> <source.tex> [<source2.tex> ...]
"""
import json, os, re, subprocess, sys

rtex, pdf, sources = sys.argv[1], os.path.abspath(sys.argv[2]), [os.path.abspath(s) for s in sys.argv[3:]]
os.chdir(os.path.dirname(pdf))

def oracle_edit(page, x, y):
    out = subprocess.run(["synctex", "edit", "-o", f"{page}:{x}:{y}:{pdf}"], capture_output=True, text=True).stdout
    f = re.search(r"^Input:(.*)$", out, re.M); l = re.search(r"^Line:(\d+)", out, re.M)
    return (os.path.normpath(f.group(1)), int(l.group(1))) if f and l else None

def rtex_inv(page, x, y):
    p = subprocess.run([rtex, "inverse", pdf, str(page), str(x), str(y), "--json"], capture_output=True, text=True)
    if p.returncode: return None
    j = json.loads(p.stdout); return (os.path.normpath(j["file"]), j["line"])

def oracle_view(src, line):
    out = subprocess.run(["synctex", "view", "-i", f"{line}:0:{src}", "-o", pdf], capture_output=True, text=True).stdout
    res = []
    for blk in out.split("Output:")[1:]:
        g = lambda k: float(re.search(rf"^{k}:([-\d.]+)", blk, re.M).group(1))
        res.append((int(g("Page")), g("h"), g("v") - g("H"), g("W"), g("H")))
    return res

def rtex_fwd(src, line):
    p = subprocess.run([rtex, "forward", pdf, src, str(line), "--json"], capture_output=True, text=True)
    return json.loads(p.stdout) if p.returncode == 0 else None

pages = int(re.search(r"Pages:\s+(\d+)", subprocess.run(["pdfinfo", pdf], capture_output=True, text=True).stdout or "Pages: 1").group(1)) if subprocess.run(["which","pdfinfo"],capture_output=True).returncode==0 else 3
inv_total = inv_same = inv_close = 0
for page in range(1, pages + 1):
    for y in range(80, 720, 16):
        for x in range(120, 500, 40):
            o = oracle_edit(page, x, y)
            if o is None: continue
            r = rtex_inv(page, x, y)
            inv_total += 1
            if r == o: inv_same += 1
            elif r and r[0] == o[0] and abs(r[1] - o[1]) <= 1: inv_close += 1
fwd_total = fwd_page = fwd_box = 0
for src in sources:
    n = sum(1 for _ in open(src, encoding="utf-8", errors="replace"))
    for line in range(1, n + 1):
        o = oracle_view(src, line)
        if not o: continue
        r = rtex_fwd(src, line)
        fwd_total += 1
        if r and r["page"] == o[0][0]:
            fwd_page += 1
            # does any rtex rect overlap the oracle's first box?
            p, h, top, w, hh = o[0]
            if any(abs(b["x"] - h) < 2 and abs(b["y"] - top) < 2 for b in r["rects"]) or \
               any(b["x"] <= h + 1 and b["y"] <= top + 1 and b["x"] + b["width"] >= h + w - 1 and b["y"] + b["height"] >= top + hh - 1 for b in r["rects"]):
                fwd_box += 1
print(f"inverse: {inv_same}/{inv_total} identical, {inv_close} more within ±1 line")
print(f"forward: {fwd_page}/{fwd_total} same page, {fwd_box}/{fwd_total} same box")
