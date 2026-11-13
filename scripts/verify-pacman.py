#!/usr/bin/env python3
"""Compare depweave's CLI output against pacman / pactree on this machine.

Read-only: only query operations and `pacman -Rsp` (print-only dry run) are used.

usage: scripts/verify-pacman.py [path/to/depweave] [--all] [--json OUT]

By default dependencies / reverse deps / removal impact are checked on a sample
of packages chosen to cover versioned deps, virtual provides and sonames. With
--all every installed package is checked.
"""

import json
import random
import re
import subprocess
import sys

ARGS = [a for a in sys.argv[1:] if not a.startswith("--")]
BIN = ARGS[0] if ARGS else "target/release/depweave"
ALL = "--all" in sys.argv
JSON_OUT = None
if "--json" in sys.argv:
    JSON_OUT = sys.argv[sys.argv.index("--json") + 1]
    if JSON_OUT in ARGS:
        ARGS.remove(JSON_OUT)
        BIN = ARGS[0] if ARGS else BIN


def run(cmd, ok_codes=(0,)):
    p = subprocess.run(cmd, capture_output=True, text=True)
    if p.returncode not in ok_codes:
        return None, p
    return p.stdout, p


def lines(s):
    return [l for l in (s or "").splitlines() if l.strip()]


def dw(*a, ok_codes=(0,)):
    out, p = run([BIN, *a], ok_codes)
    return out, p


# --------------------------------------------------------------------------
# installed package metadata from pacman -Qi (all packages in one call)
# --------------------------------------------------------------------------
def parse_qi():
    out, _ = run(["pacman", "-Qi"])
    pkgs = {}
    cur = None
    key = None
    for line in out.splitlines():
        if not line.strip():
            cur = None
            continue
        m = re.match(r"^([A-Za-z ]+?)\s*:\s?(.*)$", line)
        if m and not line.startswith(" "):
            key, val = m.group(1).strip(), m.group(2).strip()
            if key == "Name":
                cur = {"Name": val}
                pkgs[val] = cur
            elif cur is not None:
                cur[key] = val
        elif cur is not None and key:
            cur[key] = cur.get(key, "") + " " + line.strip()
    return pkgs


QI = parse_qi()
NAMES = sorted(QI)


def deplist(p):
    v = QI[p].get("Depends On", "None")
    return [] if v == "None" else v.split()


def pick_sample():
    rnd = random.Random(42)
    versioned = [p for p in NAMES if any(re.search(r"[<>=]", d) and ".so" not in d for d in deplist(p))]
    sonames = [p for p in NAMES if any(".so" in d for d in deplist(p))]
    virtual = [
        p
        for p in NAMES
        if any(re.split(r"[<>=]", d)[0] not in QI and ".so" not in d for d in deplist(p))
    ]
    providers = [p for p in NAMES if QI[p].get("Provides", "None") != "None"]
    sample = set(["glibc", "bash", "gcc-libs", "systemd", "python", "pacman"]) & set(NAMES)
    for group in (versioned, sonames, virtual, providers):
        sample.update(rnd.sample(group, min(15, len(group))))
    rest = [p for p in NAMES if p not in sample]
    sample.update(rnd.sample(rest, min(20, len(rest))))
    return sorted(sample), dict(
        versioned=len(versioned), sonames=len(sonames), virtual=len(virtual), providers=len(providers)
    )


def pactree_names(*a):
    """pactree tree output -> set of package names (first token of each line)."""
    out, _ = run(["pactree", *a], ok_codes=(0, 1))
    names = []
    for l in lines(out)[1:]:  # first line is the root
        l = re.sub(r"^[\s│├└─]+", "", l)
        # "pkg", "pkg>=1.2" (versioned literal) or "pkg provides virt=1"
        tok = re.split(r"[<>=]", l.split()[0])[0] if l.split() else ""
        if tok:
            names.append(tok)
    return set(names)


results = {}


def record(name, total, mismatches):
    results[name] = {
        "checked": total,
        "matched": total - len(mismatches),
        "mismatches": mismatches[:50],
        "mismatch_count": len(mismatches),
    }
    rate = 100.0 * (total - len(mismatches)) / total if total else 100.0
    print(f"{name:<22} {total - len(mismatches):>5}/{total:<5} {rate:6.2f}%")
    for m in mismatches[:8]:
        print(f"    {m}")


def set_check(name, pac_cmd, dw_args):
    pac, _ = run(pac_cmd, ok_codes=(0, 1))
    mine, p = dw(*dw_args)
    a, b = set(lines(pac)), set(lines(mine))
    universe = sorted(a | b)
    mism = [f"{x}: pacman={'yes' if x in a else 'no'} depweave={'yes' if x in b else 'no'}" for x in universe if (x in a) != (x in b)]
    exact = lines(pac) == lines(mine)
    record(name, len(universe), mism)
    if not mism and not exact:
        print(f"    (same set, different order/duplicates)")
        results[name]["order_mismatch"] = True


set_check("orphans (-Qdtq)", ["pacman", "-Qdtq"], ["orphans"])
set_check("explicit (-Qeq)", ["pacman", "-Qeq"], ["list", "--explicit"])
set_check("foreign (-Qmq)", ["pacman", "-Qmq"], ["list", "--foreign"])
set_check("deps-installed (-Qdq)", ["pacman", "-Qdq"], ["list", "--deps"])
set_check("unrequired (-Qtq)", ["pacman", "-Qtq"], ["list", "--unrequired"])

if ALL:
    sample, groups = NAMES, {}
else:
    sample, groups = pick_sample()
print(f"package sample: {len(sample)} {groups}")

# ---- forward deps vs pactree -d1 --------------------------------------------
mism = []
for p in sample:
    want = pactree_names("-d1", p)
    out, _ = dw("deps", p)
    got = set(lines(out))
    if want != got:
        mism.append(f"{p}: pactree-only={sorted(want - got)} depweave-only={sorted(got - want)}")
record("deps (pactree -d1)", len(sample), mism)

# ---- reverse deps vs pactree -r -d1 -------------------------------------------
mism = []
for p in sample:
    want = pactree_names("-r", "-d1", p)
    out, _ = dw("rdeps", p)
    got = set(lines(out))
    if want != got:
        mism.append(f"{p}: pactree-only={sorted(want - got)} depweave-only={sorted(got - want)}")
record("rdeps (pactree -r -d1)", len(sample), mism)

# ---- removal impact vs pacman -Rsp -------------------------------------------
mism = []
for p in sample:
    pac = subprocess.run(
        ["pacman", "-Rsp", "--print-format", "%n", p], capture_output=True, text=True
    )
    mine = subprocess.run([BIN, "impact", "--quiet", p], capture_output=True, text=True)
    pac_ok, dw_ok = pac.returncode == 0, mine.returncode == 0
    if pac_ok != dw_ok:
        mism.append(f"{p}: pacman {'ok' if pac_ok else 'refuses'}, depweave {'ok' if dw_ok else 'refuses'}")
        continue
    if pac_ok:
        a, b = set(lines(pac.stdout)), set(lines(mine.stdout))
        if a != b:
            mism.append(f"{p}: pacman-only={sorted(a - b)} depweave-only={sorted(b - a)}")
record("impact (pacman -Rsp)", len(sample), mism)

# ---- installed sizes vs pacman -Qi --------------------------------------------
mism = []
out, _ = dw("info", *NAMES, ok_codes=(0, 1))
got_sizes = {}
for block in (out or "").split("\n\n"):
    n = re.search(r"^Name +: (.*)$", block, re.M)
    s = re.search(r"^Installed Size : (.*)$", block, re.M)
    if n and s:
        got_sizes[n.group(1).strip()] = s.group(1).strip()
for p in NAMES:
    got = got_sizes.get(p)
    want = QI[p].get("Installed Size")
    if got != want:
        mism.append(f"{p}: pacman={want!r} depweave={got!r}")
record("sizes (pacman -Qi)", len(NAMES), mism)

total = sum(r["checked"] for r in results.values())
matched = sum(r["matched"] for r in results.values())
print(f"{'TOTAL':<22} {matched:>5}/{total:<5} {100.0 * matched / total:6.2f}%")
if JSON_OUT:
    with open(JSON_OUT, "w") as f:
        json.dump(results, f, indent=1)
sys.exit(0 if matched == total else 1)
