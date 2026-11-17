#!/usr/bin/env python3
"""Samples a process tree until its root exits (used by tools/cost/prove_job.sh).

    python3 -I tools/cost/rss_sampler.py ROOT_PID OUT_CSV OUT_JSON [--interval 1.0]

Every interval: the sum of VmRSS over ROOT_PID and all its descendants, the largest single VmRSS (and its command),
the 1-minute load average, MemAvailable and SwapFree. Writes one CSV row per sample and, when the root is gone, a JSON
summary with the peaks. Linux /proc only, stdlib only.
"""

import json
import os
import sys
import time


def read_kv(path):
    out = {}
    try:
        with open(path, encoding="utf-8", errors="replace") as f:
            for line in f:
                k, _, v = line.partition(":")
                out[k.strip()] = v.strip()
    except OSError:
        pass
    return out


def kib(v):
    try:
        return int(v.split()[0])
    except (ValueError, IndexError, AttributeError):
        return 0


def children_map():
    kids = {}
    for d in os.listdir("/proc"):
        if not d.isdigit():
            continue
        try:
            with open(f"/proc/{d}/stat", encoding="utf-8", errors="replace") as f:
                s = f.read()
        except OSError:
            continue
        # comm can contain spaces; fields after the closing paren are fixed.
        rest = s[s.rfind(")") + 2:].split()
        ppid = int(rest[1])
        kids.setdefault(ppid, []).append(int(d))
    return kids


def tree(root):
    kids = children_map()
    seen, todo = [], [root]
    while todo:
        p = todo.pop()
        seen.append(p)
        todo.extend(kids.get(p, []))
    return seen


def comm(pid):
    try:
        with open(f"/proc/{pid}/comm", encoding="utf-8") as f:
            return f.read().strip()
    except OSError:
        return "?"


def main():
    if len(sys.argv) < 4:
        print(__doc__, file=sys.stderr)
        return 2
    root, out_csv, out_json = int(sys.argv[1]), sys.argv[2], sys.argv[3]
    interval = float(sys.argv[sys.argv.index("--interval") + 1]) if "--interval" in sys.argv else 1.0
    peak_tree = peak_single = 0
    peak_single_comm = ""
    min_avail = min_swapfree = None
    load_max = 0.0
    load_sum = 0.0
    n = 0
    t0 = time.time()
    swap0 = kib(read_kv("/proc/meminfo").get("SwapFree", "0"))
    with open(out_csv, "w", encoding="utf-8") as f:
        f.write("t_s,tree_rss_kib,max_proc_rss_kib,max_proc,nprocs,load1,mem_available_kib,swap_free_kib\n")
        while os.path.exists(f"/proc/{root}"):
            pids = tree(root)
            total, top, top_c = 0, 0, ""
            for p in pids:
                r = kib(read_kv(f"/proc/{p}/status").get("VmRSS", "0"))
                total += r
                if r > top:
                    top, top_c = r, comm(p)
            mi = read_kv("/proc/meminfo")
            avail, swapfree = kib(mi.get("MemAvailable", "0")), kib(mi.get("SwapFree", "0"))
            with open("/proc/loadavg", encoding="utf-8") as lf:
                load1 = float(lf.read().split()[0])
            f.write(f"{time.time() - t0:.1f},{total},{top},{top_c},{len(pids)},{load1},{avail},{swapfree}\n")
            f.flush()
            if total > peak_tree:
                peak_tree = total
            if top > peak_single:
                peak_single, peak_single_comm = top, top_c
            min_avail = avail if min_avail is None else min(min_avail, avail)
            min_swapfree = swapfree if min_swapfree is None else min(min_swapfree, swapfree)
            load_max = max(load_max, load1)
            load_sum += load1
            n += 1
            time.sleep(interval)
    summary = {
        "samples": n,
        "interval_s": interval,
        "peak_tree_rss_kib": peak_tree,
        "peak_tree_rss_gib": round(peak_tree / 1048576, 3),
        "peak_single_process_rss_kib": peak_single,
        "peak_single_process": peak_single_comm,
        "min_mem_available_kib": min_avail,
        "swap_used_by_run_kib_max": (swap0 - min_swapfree) if min_swapfree is not None else None,
        "load1_mean": round(load_sum / n, 2) if n else None,
        "load1_max": load_max,
    }
    with open(out_json, "w", encoding="utf-8") as f:
        json.dump(summary, f, indent=2)
    return 0


if __name__ == "__main__":
    sys.exit(main())
