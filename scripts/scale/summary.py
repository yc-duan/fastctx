"""Compare first-screen, page-up and resource observations without hiding failed probes."""
import argparse
import json
from pathlib import Path
import statistics


def distribution(values):
    return dict(median=statistics.median(values), min=min(values), max=max(values), n=len(values)) if values else None


def summary(path):
    result = json.loads(path.read_bytes())
    operations = {}
    for sample in result["samples"]:
        operation = sample["operation"]
        bucket = operations.setdefault(operation, dict(ms=[], read_bytes=[], cpu_seconds=[], failures=[], attempts=[]))
        response = sample["result"]
        # Preserve failed work and resource costs even when there was no valid first screen.
        bucket["attempts"].append({key: value for key, value in sample.items() if key != "readings"})
        if not response.get("ok"):
            bucket["failures"].append(response.get("error", "content calibration failed"))
            if not response.get("first_screen_ok"):
                continue
        elapsed = sample.get("ms", response.get("open_ms", 0))
        counters = sample.get("node", {})
        if screen := response.get("screen"):
            elapsed = response["first_screen_ms"]
            # Match the resource boundary to first-screen latency, not the later history transfer.
            counters = response.get("node_first_screen", {})
            bucket["first_screen_bytes"] = screen["first_screen_bytes"]
            bucket["history_json_bytes"] = response.get("history_json_bytes")
            bucket["content_sha256"] = screen["content_sha256"]
            bucket["page_up_ms"] = [page["ms"] for page in screen["page_up"]]
            bucket["page_up_hashes"] = [page["content_sha256"] for page in screen["page_up"]]
        bucket["ms"].append(elapsed)
        for source, target in [("read_chars", "read_bytes"), ("cpu_seconds", "cpu_seconds")]:
            if source in counters:
                bucket[target].append(counters[source])
    for bucket in operations.values():
        for field in ("ms", "read_bytes", "cpu_seconds"):
            bucket[field] = distribution(bucket[field])
    return dict(profile=result["profile"], history=result["history"], dataset_bytes=result["dataset_bytes"],
                optimized=result["optimized"], operations=operations, quiet=result.get("quiet"),
                restarts=result.get("restarts", []), unfinished=result.get("active_operation"))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("one", type=Path)
    parser.add_argument("ten", type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    one, ten = summary(args.one), summary(args.ten)
    comparisons = {}
    for name in one["operations"].keys() | ten["operations"].keys():
        small, large = one["operations"].get(name, {}), ten["operations"].get(name, {})
        compared = dict(one=small, ten=large)
        if small.get("ms") and large.get("ms") and small["ms"]["median"]:
            compared["latency_ratio"] = large["ms"]["median"] / small["ms"]["median"]
        if small.get("read_bytes") and large.get("read_bytes") and small["read_bytes"]["median"]:
            compared["logical_read_ratio"] = large["read_bytes"]["median"] / small["read_bytes"]["median"]
        if "content_sha256" in small and "content_sha256" in large:
            compared["same_first_screen"] = small["content_sha256"] == large["content_sha256"]
            if small["page_up_hashes"] and large["page_up_hashes"]:
                compared["same_older_pages"] = small["page_up_hashes"] == large["page_up_hashes"]
        comparisons[name] = compared
    report = dict(one={k: v for k, v in one.items() if k != "operations"},
                  ten={k: v for k, v in ten.items() if k != "operations"}, comparisons=comparisons,
                  notes=["first-screen latency/resources end at GUI window JSON; painting is excluded",
                         "attempts retain full-history costs and failures; distributions require valid content",
                         "logical reads include IPC and cached reads, not just physical disk traffic",
                         "few samples describe this run; no p95 or p99 claim"])
    text = json.dumps(report, indent=2, sort_keys=True) + "\n"
    if args.output:
        args.output.write_text(text, encoding="utf-8")
    else:
        print(text, end="")


if __name__ == "__main__":
    main()
