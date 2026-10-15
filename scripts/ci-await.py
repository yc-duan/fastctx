"""Waits, in a job of a GitHub Actions run, until other jobs of the same run have uploaded what it
needs, so that it can do everything before that meanwhile. Each wanted thing is a job and the name of
the step in it that uploads it; that step having succeeded is the upload being there. A job that
ended, or a step that ended otherwise, without it fails the wait at once.

    python scripts/ci-await.py "JOB=STEP" ["JOB=STEP" ...]

Needs GH_TOKEN with read access to the run's actions, and the runner's own GITHUB_* variables.
"""
import json
import os
import subprocess
import sys
import time

# One request a poll, for every job of the run: some ten jobs waiting an hour stay well inside the
# thousand requests an hour a workflow's token is allowed.
INTERVAL = 40
LIMIT = 4 * 3600


def jobs():
    path = (f"repos/{os.environ['GITHUB_REPOSITORY']}/actions/runs/{os.environ['GITHUB_RUN_ID']}"
            "/jobs?filter=latest&per_page=100")
    done = subprocess.run(["gh", "api", path], capture_output=True, text=True)
    if done.returncode != 0:
        raise RuntimeError(done.stderr.strip() or f"gh api exited {done.returncode}")
    return {job["name"]: job for job in json.loads(done.stdout)["jobs"]}


def state(job, step):
    """'ready', 'waiting', or why it will never be ready."""
    if job is None:
        return "waiting"
    found = next((each for each in job.get("steps") or [] if each["name"] == step), None)
    if found and found.get("conclusion") == "success":
        return "ready"
    if found and found.get("status") == "completed":
        return f"its step `{step}` ended {found.get('conclusion')}"
    if job.get("status") == "completed":
        return f"the job ended {job.get('conclusion')} before `{step}`"
    return "waiting"


def main():
    wanted = [argument.split("=", 1) for argument in sys.argv[1:]]
    if not wanted or any(len(pair) != 2 for pair in wanted):
        sys.exit(__doc__)
    began = time.monotonic()
    failures = 0
    while True:
        try:
            listed = jobs()
            failures = 0
        except Exception as error:  # noqa: BLE001 - a passing API failure is asked again
            failures += 1
            if failures >= 5:
                sys.exit(f"the run's jobs could not be read: {error}")
            time.sleep(INTERVAL)
            continue
        states = {(job, step): state(listed.get(job), step) for job, step in wanted}
        broken = {key: value for key, value in states.items() if value not in ("ready", "waiting")}
        if broken:
            for (job, step), why in broken.items():
                print(f"{job}: {why}", flush=True)
            sys.exit(1)
        waiting = [job for (job, _), value in states.items() if value == "waiting"]
        if not waiting:
            print(f"ready after {round(time.monotonic() - began)}s", flush=True)
            return
        if time.monotonic() - began > LIMIT:
            sys.exit(f"still waiting for {', '.join(waiting)}")
        print(f"waiting for {', '.join(waiting)}", flush=True)
        time.sleep(INTERVAL)


if __name__ == "__main__":
    main()
