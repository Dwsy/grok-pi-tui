#!/usr/bin/env python3
"""重新生成合并分析所需的全部 /tmp 材料（只读 git 操作）。
产物：
  /tmp/merge-work/conflict_files.txt        merge-tree 冲突文件清单
  /tmp/merge-work/merge_class.json|tsv      全量逐文件分类
  /tmp/merge-work/merge3/*.diff3 + excerpts/*.txt + meta.json  冲突三方预览与摘录
  /tmp/merge-work/mergeB/*.txt + listing.json                  B1/B2 语义重叠材料
  /tmp/merge-work/batches/B1..B6.json       分析批次
"""
import subprocess, re, collections, json, os, shutil

BASE = "07b2f7144fd5c5c9d3dd1966937a87852d2dbdb8"
W = "/tmp/merge-work"
shutil.rmtree(W, ignore_errors=True)
os.makedirs(W)

def sh(*a):
    return subprocess.run(["git"] + list(a), capture_output=True, text=True).stdout

# 1) merge-tree 权威冲突清单
mt = subprocess.run(["git", "merge-tree", "--write-tree", "HEAD", "upstream/main"],
                    capture_output=True, text=True)
stage = {}
for line in mt.stdout.split("\n")[1:]:
    m = re.match(r"^([0-7]{6}) ([0-9a-f]{40}) ([123])\t(.*)$", line)
    if m:
        stage.setdefault(m.group(4), set()).add(m.group(3))
conflict = sorted(stage)
open(f"{W}/conflict_files.txt", "w").write("\n".join(conflict))
print("merge-tree exit:", mt.returncode, "conflict files:", len(conflict))

# 2) 上游 rename 图与状态
rename_src2dst, up_status = {}, {}
for line in sh("diff", "--name-status", BASE, "upstream/main").split("\n"):
    if not line.strip():
        continue
    p = line.split("\t")
    if p[0].startswith("R"):
        rename_src2dst[p[1]] = p[2]; up_status[p[2]] = "R"; up_status[p[1]] = "rsrc"
    else:
        up_status[p[1]] = p[0][0]

fork_nr = set(l for l in sh("diff", "--name-only", "--no-renames", BASE, "HEAD").split("\n") if l)
up_nr = set(l for l in sh("diff", "--name-only", "--no-renames", BASE, "upstream/main").split("\n") if l)

def numstat(a, b, path):
    out = sh("diff", "--numstat", "--no-renames", a, b, "--", path).strip()
    if not out:
        return (0, 0)
    f = out.split("\t")
    return (0 if f[0] == "-" else int(f[0]), 0 if f[1] == "-" else int(f[1]))

def hunks(a, b, path):
    out = sh("diff", "-U0", "--no-renames", a, b, "--", path)
    hs = []
    for m in re.finditer(r"^@@ -(\d+)(?:,(\d+))? ", out, re.M):
        s = int(m.group(1)); n = int(m.group(2) or 1)
        hs.append((s, s + n - 1 if n > 0 else s))
    return hs

def min_gap(h1, h2):
    best = 10**9
    for a1, a2 in h1:
        for b1, b2 in h2:
            g = 0 if (a1 <= b2 and b1 <= a2) else (b1 - a2 if a2 < b1 else a1 - b2)
            best = min(best, g)
    return best

rows = []
for path in sorted(up_nr):
    both = path in fork_nr
    r = {"path": path, "up_status": up_status.get(path, "?"),
         "rename_from": next((s for s, d in rename_src2dst.items() if d == path), None),
         "fork_touched": both, "conflict": path in stage,
         "conflict_via_rename": (path in stage and not both)}
    if both:
        r["fork_ins"], r["fork_del"] = numstat(BASE, "HEAD", path)
        if up_status.get(path) not in ("D",):
            r["up_ins"], r["up_del"] = numstat(BASE, "upstream/main", path)
            if path not in stage:
                hf, hu = hunks(BASE, "HEAD", path), hunks(BASE, "upstream/main", path)
                r["min_gap"] = min_gap(hf, hu) if hf and hu else None
    rows.append(r)
for r in rows:
    if r["conflict_via_rename"] and r.get("rename_from"):
        hf = hunks(BASE, "HEAD", r["rename_from"]); hu = hunks(BASE, "upstream/main", r["path"])
        r["fork_ins"], r["fork_del"] = numstat(BASE, "HEAD", r["rename_from"])
        r["up_ins"], r["up_del"] = numstat(BASE, "upstream/main", r["path"])
        r["min_gap"] = min_gap(hf, hu) if hf and hu else None
json.dump(rows, open(f"{W}/merge_class.json", "w"))
print("classification rows:", len(rows))

def cat(r):
    if r["conflict"] and r["fork_touched"]: return "C 文本冲突(直接)"
    if r["conflict"]: return "CR 文本冲突(经重命名)"
    if not r["fork_touched"]: return "A 上游独改"
    g = r.get("min_gap")
    if g is None: return "B0 自动合并(无法比较)"
    if g == 0: return "B1 自动合并·hunk重叠"
    if g <= 3: return "B2 自动合并·≤3行"
    if g <= 15: return "B3 自动合并·≤15行"
    return "B4 自动合并·>15行"
cnt = collections.Counter(cat(r) for r in rows)
print(dict(cnt))
with open(f"{W}/merge_class.tsv", "w") as f:
    f.write("category\tpath\tup_status\trename_from\tfork_+/-\tup_+/-\tmin_gap\n")
    for r in rows:
        c = cat(r).split()[0]
        f.write("\t".join([c, r["path"], r["up_status"], r.get("rename_from") or "",
                           f'+{r.get("fork_ins",0)}/-{r.get("fork_del",0)}',
                           f'+{r.get("up_ins",0)}/-{r.get("up_del",0)}',
                           str(r.get("min_gap") if r.get("min_gap") is not None else "")]) + "\n")

# 3) 冲突三方预览 + 摘录
os.makedirs(f"{W}/merge3/excerpts", exist_ok=True)
meta = []; idx = 0
for r in rows:
    if not r["conflict"]:
        continue
    idx += 1
    p = r["path"]; base_path = r.get("rename_from") or p
    def show(rev, path):
        return subprocess.run(["git", "show", f"{rev}:{path}"], capture_output=True)
    b = show(BASE, base_path); o = show("HEAD", base_path); t = show("upstream/main", p)
    if b.returncode or o.returncode or t.returncode:
        meta.append({"i": idx, "path": p, "error": f"blob-missing rc={b.returncode},{o.returncode},{t.returncode}",
                     "via_rename": r["conflict_via_rename"], "rename_from": r.get("rename_from"),
                     "fork_+/-": f'+{r.get("fork_ins",0)}/-{r.get("fork_del",0)}',
                     "up_+/-": f'+{r.get("up_ins",0)}/-{r.get("up_del",0)}'})
        continue
    bf, of, tf = f"{W}/merge3/{idx}.base", f"{W}/merge3/{idx}.ours", f"{W}/merge3/{idx}.theirs"
    open(bf, "wb").write(b.stdout); open(of, "wb").write(o.stdout); open(tf, "wb").write(t.stdout)
    m = subprocess.run(["git", "merge-file", "-p", "--diff3", "-L", "ours(fork)", "-L", "base", "-L", "theirs(upstream)",
                        of, bf, tf], capture_output=True)
    merged = m.stdout.decode("utf-8", "replace") if m.stdout else ""
    nconf = merged.count("<<<<<<<")
    open(f"{W}/merge3/{idx}.diff3", "w").write(merged)
    CTX = 7
    lines = merged.split("\n")
    blocks = []
    for i, l in enumerate(lines):
        if l.startswith("<<<<<<<"): blocks.append([i, i])
        elif blocks and blocks[-1][1] == i - 1 and l[:7] in ("|||||||", "=======", ">>>>>>>"):
            blocks[-1][1] = i
    mg = []
    for bl in blocks:
        if mg and bl[0] - mg[-1][1] <= 2 * CTX: mg[-1][1] = bl[1]
        else: mg.append(bl)
    out = [f"# conflict excerpt for {p}  (blocks: {nconf}, fork +{r.get('fork_ins',0)}/-{r.get('fork_del',0)}, upstream +{r.get('up_ins',0)}/-{r.get('up_del',0)}" +
           (f", via rename from {r['rename_from']}" if r.get("rename_from") else "") + ")"]
    if not blocks:
        out.append("# (no conflict blocks found — file-level conflict)")
    for s, e in mg:
        a = max(0, s - CTX); bnd = min(len(lines), e + CTX + 1)
        out.append(f"\n# ---- lines {a+1}-{bnd} ----")
        out.extend(lines[a:bnd])
    ep = f"{W}/merge3/excerpts/{idx:03d}.txt"
    open(ep, "w").write("\n".join(out))
    meta.append({"i": idx, "path": p, "via_rename": r["conflict_via_rename"], "rename_from": r.get("rename_from"),
                 "fork_+/-": f'+{r.get("fork_ins",0)}/-{r.get("fork_del",0)}',
                 "up_+/-": f'+{r.get("up_ins",0)}/-{r.get("up_del",0)}',
                 "conflict_hunks": nconf, "size": len(merged), "excerpt": ep, "excerpt_lines": len(out)})
json.dump(meta, open(f"{W}/merge3/meta.json", "w"), indent=1)
print("diff3 conflicts:", len(meta), "total hunks:", sum(m.get("conflict_hunks", 0) for m in meta))

# 4) B1/B2 材料与批次
os.makedirs(f"{W}/mergeB", exist_ok=True)
b12 = [r for r in rows if not r["conflict"] and r["fork_touched"] and r.get("min_gap") is not None and r["min_gap"] <= 3]
b12.sort(key=lambda r: (r["min_gap"], r["path"]))
listing = []; idx = 0
for r in b12:
    idx += 1
    p = r["path"]
    f = sh("diff", "-U2", "--no-renames", BASE, "HEAD", "--", p)
    u = sh("diff", "-U2", "--no-renames", BASE, "upstream/main", "--", p)
    fn = f"{W}/mergeB/{idx:03d}.txt"
    with open(fn, "w") as fh:
        fh.write(f"### FILE: {p}\n### min_gap(base坐标)={r['min_gap']}  fork +{r.get('fork_ins',0)}/-{r.get('fork_del',0)}  upstream +{r.get('up_ins',0)}/-{r.get('up_del',0)}\n\n")
        fh.write("===== FORK DIFF (base -> HEAD) =====\n" + f)
        fh.write("\n===== UPSTREAM DIFF (base -> upstream/main) =====\n" + u)
    listing.append({"i": idx, "path": p, "gap": r["min_gap"], "file": fn})
json.dump(listing, open(f"{W}/mergeB/listing.json", "w"), indent=1)
print("b12 items:", len(listing))

def batch_for_conflict(p):
    if p == "Cargo.lock": return "B6"
    if "/xai-grok-pager-pty-harness/" in p or p.startswith("crates/codegen/xai-grok-pager/tests/") or "/tests/" in p or p.endswith("_tests.rs"): return "B5"
    if not any(c in p for c in ("/xai-grok-pager/", "/xai-grok-pager-render/", "/xai-grok-pager-minimal/", "/xai-grok-pager-bin/", "/xai-grok-pager-diff/", "/xai-grok-pager-pty-harness/")): return "B6"
    if "/app/agent_view" in p or "/session" in p or "queue" in p.lower(): return "B2"
    if "/app/" in p: return "B1"
    if "/views/" in p or "/scrollback/" in p: return "B3"
    return "B4a"

def batch_for_b12(p):
    if "/xai-grok-pager/" not in p: return "B6"
    if "/app/agent_view" in p or "/session" in p or "queue" in p.lower(): return "B2"
    if "/app/" in p: return "B1"
    if "/views/" in p or "/scrollback/" in p: return "B3"
    return "B4b"

batches = {}
for m in meta:
    batches.setdefault(batch_for_conflict(m["path"]), []).append(
        {"kind": "conflict", "i": m["i"], "path": m["path"], "excerpt": m.get("excerpt"),
         "hunks": m.get("conflict_hunks"), "fork": m.get("fork_+/-"), "up": m.get("up_+/-"),
         "via_rename": m.get("via_rename", False), "error": m.get("error")})
for l in listing:
    batches.setdefault(batch_for_b12(l["path"]), []).append(
        {"kind": "b12", "i": l["i"], "path": l["path"], "gap": l["gap"], "diffs": l["file"]})
os.makedirs(f"{W}/batches", exist_ok=True)
for b, items in sorted(batches.items()):
    json.dump(items, open(f"{W}/batches/{b}.json", "w"), indent=1, ensure_ascii=False)
    print(b, len(items))
