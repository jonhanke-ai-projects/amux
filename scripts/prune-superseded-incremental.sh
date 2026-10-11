#!/usr/bin/env bash
# Prune incremental caches of SUPERSEDED crate fingerprints after a build
# (Mac disk RCA 20261011-112249).
#
# cargo writes target/<profile>/incremental/<crate>-<hash>/ per fingerprint
# and never deletes the old one. A profile, toolchain or flag change (87e776bc
# changed the deploy profile's strip setting) starts a new tree, and the deploy
# cache held four amux_server trees, one of them 9 GB, last used before the
# change. For each crate the build WROTE (a tree touched since <since_epoch>),
# its older sibling trees are dead and are removed. Three guards, because a
# name is not one crate: a crate the build did not compile is left alone; the
# two newest trees per name are always kept (amux-server's lib and bin are both
# `amux_server`, and a bin-only rebuild leaves the lib tree untouched); and a
# tree is removed only when idle AMUX_INCREMENTAL_IDLE_H hours (default 2).
# build_script_build-* names belong to every package with a build.rs, so they
# are never grouped.
#
# Usage: prune-superseded-incremental.sh <incremental_dir> <since_epoch>
# Prints one verdict line: incremental_pruned / incremental_none.
set -euo pipefail
DIR="${1:?incremental dir}"; SINCE="${2:?since epoch}"
[ -d "$DIR" ] || { echo "incremental: $DIR missing (verdict=incremental_none)"; exit 0; }
python3 - "$DIR" "$SINCE" <<'PY'
import os, re, shutil, sys
import time
d, since = sys.argv[1], float(sys.argv[2])
idle_s = float(os.environ.get("AMUX_INCREMENTAL_IDLE_H", "2")) * 3600
now = time.time()
def newest(p):
    m = 0.0
    for root, _, files in os.walk(p):
        for f in files:
            try: m = max(m, os.lstat(os.path.join(root, f)).st_mtime)
            except OSError: pass
    return m
def size(p):
    t = 0
    for root, _, files in os.walk(p):
        for f in files:
            try: t += os.lstat(os.path.join(root, f)).st_size
            except OSError: pass
    return t
groups = {}
for name in os.listdir(d):
    p = os.path.join(d, name)
    m = re.match(r'^(.*)-([0-9a-z]{10,})$', name)
    if m and os.path.isdir(p) and not m.group(1).startswith("build_script_"): groups.setdefault(m.group(1), []).append(p)
n = freed = 0
for crate, dirs in groups.items():
    age = {p: newest(p) for p in dirs}
    if not any(t >= since for t in age.values()): continue
    keep = set(sorted(dirs, key=lambda p: age[p], reverse=True)[:2])
    for p in dirs:
        if p in keep or age[p] >= since or now - age[p] < idle_s: continue
        b = size(p)
        shutil.rmtree(p, ignore_errors=True)
        if not os.path.exists(p): n += 1; freed += b
if n:
    print(f"incremental: pruned {n} superseded tree(s), {freed/2**30:.1f}G (verdict=incremental_pruned dir={d})")
else:
    print(f"incremental: nothing superseded (verdict=incremental_none dir={d})")
PY
