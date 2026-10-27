#!/usr/bin/env python3
"""Build-time comparison: rtex vs latexmk (what TeXShop's pdflatexmk engine runs).

usage: bench_build.py <rtex binary> <doc.tex> [runs]
"""
import os, re, shutil, subprocess, sys, time

rtex, doc = os.path.abspath(sys.argv[1]), os.path.abspath(sys.argv[2])
runs = int(sys.argv[3]) if len(sys.argv) > 3 else 3
os.chdir(os.path.dirname(doc))
name = os.path.basename(doc)
stem = name[:-4]
cache = os.path.join(os.environ.get("TMPDIR", "/tmp"), "rtex-bench-cache")
env = dict(os.environ, RTEX_CACHE_DIR=cache)
original = open(doc).read()

LATEXMK = ["latexmk", "-pdf", "-synctex=1", "-interaction=nonstopmode", "-file-line-error", name]

def clean():
    subprocess.run(["latexmk", "-C", name], capture_output=True)
    subprocess.run([rtex, "clean", name], capture_output=True, env=env)
    shutil.rmtree(cache, ignore_errors=True)

def timed(cmd):
    t = time.perf_counter()
    subprocess.run(cmd, capture_output=True, env=env)
    return time.perf_counter() - t

def edit(i, label=False):
    extra = f"\n\\section{{Bench {i}}}\\label{{bench:{i}}}" if label else ""
    text = original.replace("\\end{document}", f"Edit number {i}.{extra}\n\\end{{document}}")
    open(doc, "w").write(text)

def scenario(title, setup, cmd, n=runs):
    times = []
    for i in range(n):
        setup(i)
        times.append(timed(cmd))
    times.sort()
    return times[len(times) // 2]

rows = []
try:
    for label, cmd in [("latexmk (TeXShop pdflatexmk)", LATEXMK),
                       ("rtex build", [rtex, "build", name]),
                       ("rtex build --fast", [rtex, "build", "--fast", name])]:
        fresh = scenario("fresh", lambda i: (open(doc, "w").write(original), clean()), cmd)
        # Warm: run once so aux files/format exist, then measure edits.
        open(doc, "w").write(original); clean(); subprocess.run(cmd, capture_output=True, env=env)
        small = scenario("edit", lambda i: edit(100 + i), cmd)
        labels = scenario("label", lambda i: edit(200 + i, label=True), cmd)
        rows.append((label, fresh, small, labels))
finally:
    open(doc, "w").write(original)

print(f"{'':32} {'fresh build':>12} {'text edit':>12} {'new label':>12}")
for label, a, b, c in rows:
    print(f"{label:32} {a:11.2f}s {b:11.2f}s {c:11.2f}s")
pages = re.search(r"\((\d+) pages?", open(f"{stem}.log", errors="replace").read())
print(f"document: {name}, {pages.group(1) if pages else '?'} pages; median of {runs} runs")
