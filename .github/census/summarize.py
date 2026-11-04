"""The census's read: per-test failure counts with 95% Wilson intervals.

summarize.py DIR: DIR holds each job's artifact (solo-<shard>/solo.csv,
workspace-<shard>/logs/ws-<pass>-<arm>.log). Writes DIR/summary.md and two
per-test tables beside it, and exits 1 if any solo run was INVALID (the
census itself was wrong, not a test).
"""
import collections
import csv
import glob
import math
import os
import re
import sys

Z = 1.959963984540054
ARM = {"a": "#80 c0a07d5", "b": "#81 9b57a31"}


def wilson(k, n):
    if n == 0:
        return (float("nan"), float("nan"))
    p = k / n
    d = 1 + Z * Z / n
    c = (p + Z * Z / (2 * n)) / d
    h = Z * math.sqrt(p * (1 - p) / n + Z * Z / (4 * n * n)) / d
    return (max(0.0, c - h), min(1.0, c + h))


def solo(root):
    by = collections.defaultdict(collections.Counter)
    detail = collections.defaultdict(list)
    for path in sorted(glob.glob(os.path.join(root, "solo-*", "solo.csv"))):
        for r in csv.DictReader(open(path)):
            key = (r["binary"], r["test"], r["arm"])
            by[key][r["verdict"]] += 1
            if r["verdict"] not in ("PASS", "SKIPPED"):
                detail[key].append(f"pass {r['pass']}: {r['verdict']} {r['detail'].strip()}")
    return by, detail


RUNNING = re.compile(r"^\s+Running (\S+) \((?:.*/)?([A-Za-z0-9_]+?)-[0-9a-f]+\)")
DOCTESTS = re.compile(r"^\s+Doc-tests (\S+)")
RESULT = re.compile(r"^test (\S+) \.\.\. (ok|FAILED|ignored)\s*$")
SUMMARY = re.compile(r"^test result: ")


def workspace(root):
    by = collections.defaultdict(collections.Counter)
    broken = collections.defaultdict(list)
    runs = collections.Counter()
    for path in sorted(glob.glob(os.path.join(root, "workspace-*", "logs", "ws-*-*.log"))):
        m = re.search(r"ws-(\d+)-([ab])\.log$", path)
        if not m:
            continue
        shard = path.split(os.sep)[-3]
        arm = m.group(2)
        runs[arm] += 1
        cur, closed = None, True
        killed = False
        for line in open(path, errors="replace"):
            line = line.rstrip("\n")
            if line.startswith("KILLED:"):
                killed = True
            r = RUNNING.match(line) or DOCTESTS.match(line)
            if r:
                if cur and not closed:
                    broken[(cur, arm)].append(f"{shard} pass {m.group(1)}: no test result")
                cur = f"{r.group(2)}:{r.group(1)}" if r.re is RUNNING else f"doctests:{r.group(1)}"
                closed = False
                continue
            t = RESULT.match(line)
            if t and cur:
                by[(cur, t.group(1), arm)][t.group(2)] += 1
            elif SUMMARY.match(line):
                closed = True
        if cur and not closed:
            broken[(cur, arm)].append(f"{shard} pass {m.group(1)}: " + ("killed by the timer" if killed else "no test result"))
    return by, broken, runs


def main(root):
    out = []
    sby, sdetail = solo(root)
    wby, wbroken, wruns = workspace(root)
    rows = []
    invalid = 0
    for (b, t, arm), c in sorted(sby.items()):
        n = c["PASS"] + c["FAIL"] + c["TIMEOUT"] + c["ERROR"]
        k = c["FAIL"] + c["TIMEOUT"] + c["ERROR"]
        lo, hi = wilson(k, n)
        invalid += c["INVALID"]
        rows.append(("solo", b, t, arm, k, n, lo, hi, c["SKIPPED"], c["INVALID"], dict(c)))
    for (b, t, arm), c in sorted(wby.items()):
        n = c["ok"] + c["FAILED"]
        k = c["FAILED"]
        lo, hi = wilson(k, n)
        rows.append(("workspace", b, t, arm, k, n, lo, hi, 0, 0, dict(c)))
    with open(os.path.join(root, "per-test.csv"), "w", newline="") as f:
        w = csv.writer(f)
        w.writerow(["where", "binary", "test", "arm", "failed", "runs", "wilson_lo", "wilson_hi", "skipped", "invalid", "verdicts"])
        for r in rows:
            w.writerow([*r[:6], f"{r[6]:.3f}", f"{r[7]:.3f}", r[8], r[9], r[10]])

    out.append("# Census on macos-15: per-test failure counts, 95% Wilson intervals\n")
    out.append(f"Arms: a = {ARM['a']}, b = {ARM['b']}. Workspace passes per arm: "
               + ", ".join(f"{a} {wruns[a]}" for a in sorted(wruns)) + ".\n")
    solo_runs = sum(sum(c.values()) for c in sby.values())
    verdicts = collections.Counter()
    for c in sby.values():
        verdicts.update(c)
    out.append(f"Solo: {len(sby)} test-arm pairs, {solo_runs} runs; verdicts {dict(verdicts)}.\n")
    out.append("\n## Every test that failed at least once\n")
    out.append("| where | binary | test | arm | failed / runs | Wilson 95% |")
    out.append("|---|---|---|---|---|---|")
    fragile = [r for r in rows if r[4] > 0]
    for r in sorted(fragile, key=lambda r: (-r[4] / r[5], r[1], r[2], r[0], r[3])):
        out.append(f"| {r[0]} | {r[1]} | {r[2]} | {r[3]} | {r[4]} / {r[5]} | [{r[6]:.3f}, {r[7]:.3f}] |")
    if not fragile:
        out.append("| (none) | | | | | |")
    out.append("\nThe same tests, on the other arm and in the other mode, are in per-test.csv.\n")
    out.append("\n## Binaries that ended without a test result (crash or kill)\n")
    for (b, arm), why in sorted(wbroken.items()):
        out.append(f"- {b} arm {arm}: " + "; ".join(why))
    if not wbroken:
        out.append("- none")
    out.append("\n## Solo runs that skipped (a SKIP line other than `SKIP normative:`)\n")
    sk = [(b, t, arm, c["SKIPPED"]) for (b, t, arm), c in sorted(sby.items()) if c["SKIPPED"]]
    for b, t, arm, n in sk:
        out.append(f"- {b} {t} arm {arm}: {n}")
    if not sk:
        out.append("- none")
    out.append("\n## Failure details (solo)\n")
    for key, lines in sorted(sdetail.items()):
        out.append(f"- {key[0]} {key[1]} arm {key[2]}:")
        for l in lines:
            out.append(f"  - {l}")
    if invalid:
        out.append(f"\n**{invalid} INVALID solo runs: the census is wrong somewhere, not a test.**")
    text = "\n".join(out) + "\n"
    open(os.path.join(root, "summary.md"), "w").write(text)
    print(text)
    return 1 if invalid else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1]))
