#!/usr/bin/env python3
"""Milestone 0 speed baseline (spec §19, item 4): open, Make cut-ready and plan each design of a
local acceptance manifest with `cutcraft-cli plan`, one process per design, and record the wall
time, CPU time and peak memory of each.

    scripts/m0-speed.py CLI MANIFEST.json OUT.jsonl [--repeat N]

Paths in the manifest are relative to CUTCRAFT_CORPUS_DIR, as for `cutcraft-cli corpus`. The
manifest, the designs and the results are private: keep OUT in the gitignored local/ folder.
A summary (counts and percentiles only, no names) is printed at the end.
"""

import json
import os
import subprocess
import sys
import tempfile
import time


def run_one(cli, design, recipe, out_dir):
    args = [cli, "plan", design, "--out", out_dir]
    recipe_file = None
    if recipe:
        recipe_file = tempfile.NamedTemporaryFile("w", suffix=".json", delete=False)
        json.dump(recipe, recipe_file)
        recipe_file.close()
        args += ["--recipe", recipe_file.name]
    start = time.monotonic()
    proc = subprocess.Popen(args, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    # wait4 gives this child's own resource use (peak memory included), not all children's.
    _, status, usage = os.wait4(proc.pid, 0)
    wall = time.monotonic() - start
    err = proc.stderr.read().decode("utf-8", "replace") if proc.stderr else ""
    if recipe_file:
        os.unlink(recipe_file.name)
    return {
        "ok": os.waitstatus_to_exitcode(status) == 0,
        "wall_s": round(wall, 3),
        "cpu_s": round(usage.ru_utime + usage.ru_stime, 3),
        "max_rss_mb": round(usage.ru_maxrss / 1024, 1),
        "error": err.strip().splitlines()[-1][:300] if err.strip() and os.waitstatus_to_exitcode(status) != 0 else None,
    }


def percentile(values, p):
    if not values:
        return None
    values = sorted(values)
    k = (len(values) - 1) * p / 100
    lo, hi = int(k), min(int(k) + 1, len(values) - 1)
    return round(values[lo] + (values[hi] - values[lo]) * (k - lo), 3)


def main():
    args = sys.argv[1:]
    repeat = 1
    if "--repeat" in args:
        i = args.index("--repeat")
        repeat = int(args[i + 1])
        del args[i : i + 2]
    if len(args) != 3:
        sys.exit(__doc__)
    cli, manifest_path, out_path = args
    corpus = os.environ.get("CUTCRAFT_CORPUS_DIR")
    if not corpus:
        sys.exit("set CUTCRAFT_CORPUS_DIR")
    entries = json.load(open(manifest_path))["entries"]

    # Start-up alone: the program's own fixed cost (fonts, engine), measured with --help.
    startup = []
    for _ in range(5):
        start = time.monotonic()
        subprocess.run([cli, "--help"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        startup.append(time.monotonic() - start)

    results = []
    with open(out_path, "w") as out, tempfile.TemporaryDirectory() as scratch:
        for n, entry in enumerate(entries):
            design = os.path.join(corpus, entry["path"])
            for r in range(repeat):
                result = run_one(cli, design, entry.get("recipe"), os.path.join(scratch, f"{n}-{r}"))
                result.update({"path": entry["path"], "run": r})
                out.write(json.dumps(result) + "\n")
                out.flush()
                results.append(result)
            print(f"{n + 1}/{len(entries)}", end="\r", file=sys.stderr)

    ok = [r for r in results if r["ok"]]
    walls = [r["wall_s"] for r in ok]
    rss = [r["max_rss_mb"] for r in ok]
    summary = {
        "designs": len(entries),
        "runs": len(results),
        "failed": len(results) - len(ok),
        "startup_s_median": percentile(startup, 50),
        "wall_s": {p: percentile(walls, p) for p in (50, 90, 99, 100)},
        "max_rss_mb": {p: percentile(rss, p) for p in (50, 90, 99, 100)},
        "over_10s": sum(1 for w in walls if w > 10),
    }
    print(json.dumps(summary, indent=2))


if __name__ == "__main__":
    main()
