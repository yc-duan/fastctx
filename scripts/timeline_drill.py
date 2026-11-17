"""Replay offline host records through a real node's timeline and compare its reference history.

Build: cargo build -p fastctx --features fastctx-node/timeline-drill
Run: python scripts/timeline_drill.py --kernel PATH/TO/fastctx.exe --root ABSOLUTE_DIRECTORY_THAT_DOES_NOT_EXIST

The optional feature adds only a read-only reference-history probe and zero-write build logging.
No real host or model login is used. This Windows drill never installs a service or joins a hub.
"""

import argparse
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timedelta, timezone
import hashlib
import json
from pathlib import Path
import statistics
import sys
import time
import traceback

from timeline_lab import CallFailed, Lab, put_json, until


START = datetime(2026, 11, 10, tzinfo=timezone.utc)


def timestamp(index):
    return (START + timedelta(milliseconds=index * 100)).isoformat().replace("+00:00", "Z")


def rows(count, prefix="row", start=0, parent=None):
    for index in range(start, start + count):
        identity = f"{prefix}-{index:08d}"
        user = index % 2 == 0
        text = f"fixture {identity}: {'question' if user else 'answer'} 中文"
        yield {"type": "user" if user else "assistant", "uuid": identity, "parentUuid": parent,
               "timestamp": timestamp(index), "sessionId": "offline-fixture",
               "message": {"role": "user" if user else "assistant",
                           "content": text if user else [{"type": "text", "text": text}]}}
        parent = identity


class Drill:
    def __init__(self, lab):
        self.lab = lab
        self.results = []

    def result(self, label, passed, **detail):
        value = {"case": label, "passed": bool(passed), **detail}
        self.results.append(value)
        print(json.dumps(value, ensure_ascii=False), flush=True)
        put_json(self.lab.root / "results.json", self.results)
        return passed

    def compare(self, label, resident, build=None):
        reference = self.lab.golden(resident)
        paged, page_info = self.lab.pages(resident)
        after = self.lab.golden(resident)
        stable = reference["stamp_before"] == after["stamp_after"]
        expected = reference["history"]
        mismatch = next((index for index, pair in enumerate(zip(expected, paged)) if pair[0] != pair[1]), None)
        encode = lambda value: json.dumps(value, sort_keys=True, ensure_ascii=False, separators=(",", ":"))
        encoded = encode(expected)
        same = encoded == encode(paged) and stable
        self.lab.evidence(label, {"reference": reference, "paged": paged, "page_info": page_info,
                                  "stable": stable, "build": build})
        digest = hashlib.sha256(encoded.encode("utf-8")).hexdigest()
        self.result(label, same, expected=len(expected), actual=len(paged), stable=stable,
                    first_mismatch=mismatch, **page_info, sha256=digest, build=build)
        return expected

    def build_compare(self, label, resident, action=None):
        build = self.lab.settled_build(resident, action)
        return self.compare(label, resident, build)

    def benchmark(self, resident, samples=31):
        times = []
        size = 0
        with self.lab.connect() as client:
            for index in range(samples + 3):
                started = time.perf_counter()
                reply = client.call({"call": "timeline", "machine": None, "request": {"do": "page",
                    "resident": resident, "from": {"from": "latest"}, "max_events": 400, "max_bytes": 1048576}})
                elapsed = (time.perf_counter() - started) * 1000
                size = len(reply["events"])
                if index >= 3:
                    times.append(elapsed)
        ordered = sorted(times)
        self.result(f"latency-{resident}", True, history_events=self.lab.stats(resident)["items"],
                    page_events=size, samples=samples, median_ms=statistics.median(times),
                    p95_ms=ordered[int((len(times) - 1) * .95)], max_ms=max(times), raw_ms=times,
                    build_profile="dev, debug=0", transport="existing local IPC connection")

    def run(self, large_events):
        lab = self.lab
        main = "claude-dd11"
        empty = "claude-ee00"
        large = "claude-aa99"
        data = list(rows(1000))
        lab.seed(main, data)
        lab.seed(empty, [])
        lab.seed(large, rows(large_events, "large"))
        lab.start()
        self.result("fresh-content", lab.stats(main)["revision"] is None, stats=lab.stats(main))
        self.build_compare("empty-first-build", empty)
        self.build_compare("first-build", main)
        self.benchmark(main)

        build = lab.settled_build(main)
        self.result("unchanged-write", build["before"] == build["after"]
                    and all(written == gone == 0 for _, written, gone in build["builds"]), **build)

        appended = list(rows(2, start=len(data), parent=data[-1]["uuid"]))
        data.extend(appended)
        old_stats = lab.stats(main)
        self.build_compare("append-round", main, lambda: lab.rows(main, data))
        stats = lab.stats(main)
        self.result("append-write-count", stats["records"] - old_stats["records"] == 2,
                    appended_events=2, new_records=stats["records"] - old_stats["records"])

        data[501]["message"]["content"][0]["text"] = "changed middle answer 中间改了"
        self.build_compare("middle-change", main, lambda: lab.rows(main, data))
        del data[500]
        data[500]["parentUuid"] = data[499]["uuid"]
        self.build_compare("middle-delete", main, lambda: lab.rows(main, data))

        data.insert(300, {"type": "assistant", "uuid": "inserted-middle", "parentUuid": data[299]["uuid"],
            "timestamp": (START + timedelta(milliseconds=29950)).isoformat().replace("+00:00", "Z"),
            "message": {"role": "assistant", "content": [{"type": "text", "text": "inserted in the middle"}]}})
        self.build_compare("middle-insert", main, lambda: lab.rows(main, data))

        extra = list(rows(2, prefix="oversize", start=2000, parent=data[-1]["uuid"]))
        extra[1]["message"]["content"][0]["text"] = "大对象🙂" * 50000
        data.extend(extra)
        self.build_compare("oversized-object", main, lambda: lab.rows(main, data))

        compact_at = 3000
        data.extend([
            {"type": "system", "subtype": "compact_boundary", "uuid": "boundary", "parentUuid": data[-1]["uuid"],
             "timestamp": timestamp(compact_at), "compactMetadata": {"trigger": "manual", "preTokens": 10000}},
            {"type": "user", "uuid": "summary", "parentUuid": "boundary", "timestamp": timestamp(compact_at + 1),
             "isCompactSummary": True, "message": {"role": "user", "content": "Summary of the earlier work."}}])
        self.build_compare("compaction-boundary", main, lambda: lab.rows(main, data))

        # Replacing the host file represents a persisted rollback/crop, without invoking a model.
        data = data[:200] + data[-2:]
        data[-2]["parentUuid"] = data[199]["uuid"]
        self.build_compare("cropped-history", main, lambda: lab.rows(main, data))

        before = lab.stats(main)
        lab.stop()
        lab.start()
        self.compare("restart-before-rebuild", main)
        build = lab.settled_build(main)
        self.result("restart-no-rewrite", before == build["after"], **build)

        revision = lab.stats(main)["revision"]
        started = time.perf_counter()
        reply = lab.timeline(main, "moved", after=revision)
        elapsed = time.perf_counter() - started
        self.result("moved-no-change", reply["revision"] == revision and 24 <= elapsed < 30,
                    before=revision, reply=reply, elapsed_s=elapsed)

        build_count = len(lab.builds(main))
        revision = lab.stats(main)["revision"]
        with ThreadPoolExecutor(max_workers=1) as executor:
            began = time.perf_counter()
            future = executor.submit(lab.timeline, main, "moved", after=revision)
            time.sleep(1)
            data.extend(rows(2, prefix="moved", start=4000, parent=data[-1]["uuid"]))
            changed_at = time.perf_counter()
            lab.rows(main, data)
            lab.timeline(main)
            moved = future.result(timeout=40)
        until(lambda: len(lab.builds(main)) > build_count and not lab.jobs(main), 60)
        elapsed = time.perf_counter() - began
        self.result("moved-change", moved["revision"] is not None and moved["revision"] > revision,
                    before=revision, reply=moved, elapsed_s=elapsed,
                    after_fixture_change_s=time.perf_counter() - changed_at,
                    build=lab.builds(main)[build_count:])
        self.compare("after-moved", main)

        began = time.perf_counter()
        self.build_compare("large-history", large)
        self.result("large-drill-time", True, elapsed_s=time.perf_counter() - began,
                    host_file_bytes=lab.fixtures[large].stat().st_size)
        self.benchmark(large)

        self.delete(main)

    def deletion_only(self):
        resident = "claude-de11"
        data = list(rows(20))
        data[-1]["message"]["content"][0]["text"] = "retained-object-fixture " * 10000
        self.lab.seed(resident, data)
        self.lab.start()
        self.build_compare("delete-source", resident)
        self.delete(resident)
        evidence = self.lab.deleted_content
        self.result("delete-content-purged", all(value == 0 for value in evidence["physical"]["counts"].values()),
                    **evidence["physical"])
        self.result("delete-object-files", all(item["passed"] for item in evidence["objects"]),
                    objects=evidence["objects"])

    def delete(self, resident, label="delete"):
        lab = self.lab
        # A completed build makes the next request wait for the normal throttle.
        lab.settled_build(resident)
        lab.timeline(resident)
        until(lambda: bool(lab.jobs(resident)), 5)
        pending = len(lab.jobs(resident))
        objects = lab.referenced_objects(resident)
        deleted_at_ms = int(time.time() * 1000)
        response = lab.call({"call": "forget", "resident": resident, "machine": "timeline-drill"})
        try:
            after_delete = lab.timeline(resident)
        except CallFailed as error:
            after_delete = {"failure": error.failure}
        before_wait = len(lab.builds(resident))
        began = time.perf_counter()
        settled = True
        try:
            until(lambda: not lab.content_forget_jobs(resident), 60)
        except TimeoutError:
            settled = False
        waited = time.perf_counter() - began
        if waited < 16:
            time.sleep(16 - waited)
        later = lab.jobs(resident)
        physical = lab.content_rows(resident)
        remaining_objects = lab.object_facts([item["hash"] for item in objects])
        for before, after in zip(objects, remaining_objects):
            age = deleted_at_ms - before["made_ms"] if before["made_ms"] is not None else None
            after.update(age_at_delete_ms=age, allowed_to_remain=age is not None and age < 60000)
            after["passed"] = after["allowed_to_remain"] or (not after["stored"] and not after["file_exists"])
        forget_jobs = lab.content_forget_jobs(resident)
        lab.deleted_content = {"physical": physical, "objects": remaining_objects, "forget_jobs_after": forget_jobs}
        put_json(lab.root / "deleted-content.json", lab.deleted_content)
        self.result(label, response.get("reply") == "forgetting" and response.get("how") == "deleted"
                    and after_delete.get("failure", {}).get("code") == "deleted"
                    and not later and len(lab.builds(resident)) == before_wait and settled
                    and all(value == 0 for value in physical["counts"].values()),
                    queued_before=pending, response=response, read_after=after_delete,
                    jobs_after=later, builds_after_delete=len(lab.builds(resident)) - before_wait,
                    forget_settled=settled, forget_waited_s=waited, forget_jobs_after=forget_jobs,
                    remaining_content=lab.stats(resident), physical=physical)


def main():
    sys.stdout.reconfigure(encoding="utf-8")
    sys.stderr.reconfigure(encoding="utf-8")
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--kernel", type=Path, required=True)
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--large-events", type=int, default=100000)
    family = parser.add_mutually_exclusive_group()
    family.add_argument("--fake-codex", type=Path, help="run the Codex scenarios using a private copy of this fake host")
    family.add_argument("--delete-only", action="store_true", help="minimal deletion and retained-content reproduction")
    args = parser.parse_args()
    if sys.platform != "win32":
        parser.error("this drill exercises the Windows development node")
    lab = Lab(args.kernel, args.root)
    drill = Drill(lab)
    try:
        if args.delete_only:
            drill.deletion_only()
        elif args.fake_codex:
            from timeline_codex_drill import run
            run(drill, args.fake_codex)
        else:
            drill.run(args.large_events)
    except Exception as error:
        traceback.print_exc()
        drill.result("drill-exception", False, exception=repr(error), traceback=traceback.format_exc())
    finally:
        lab.cleanup()
    passed = all(result["passed"] for result in drill.results)
    print(json.dumps({"passed": passed, "cases": len(drill.results), "root": str(lab.root)}), flush=True)
    return 0 if passed else 1


if __name__ == "__main__":
    raise SystemExit(main())
