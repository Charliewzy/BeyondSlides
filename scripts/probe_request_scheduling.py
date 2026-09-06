# /// script
# requires-python = ">=3.12"
# dependencies = ["httpx>=0.28,<0.29"]
# ///
"""Isolated scheduling experiment. Replays comparisons, never production checkpoints.

The simulator and live runs share the same candidate policy. Fixed modes model
the previous independent retry behavior; adaptive modes coordinate cooldown.
"""
from __future__ import annotations

import argparse
import asyncio
import heapq
import json
import os
import random
import statistics
import sys
import time
from collections import Counter
from dataclasses import dataclass
from pathlib import Path

import httpx


@dataclass
class Controller:
    mode: str
    ceiling: int = 8
    cap: int = 2
    spacing: float = 0
    next_start: float = 0
    cooldown: float = 0
    generation: int = 0
    successes: int = 0
    latency: float = 1
    pressure: bool = False
    paced_pressure: bool = False
    epoch_started: float | None = None
    peak_cap: int = 2

    def __post_init__(self):
        if self.mode.startswith("fixed"):
            self.cap = int(self.mode.removeprefix("fixed"))
        self.peak_cap = self.cap

    def admit(self, now, active):
        if self.epoch_started is None:
            self.epoch_started = now
        self.next_start = now + self.spacing
        return self.generation

    def finish(self, now, generation, status, elapsed, retry_after=0):
        if self.mode.startswith("fixed"):
            return
        if status == 429:
            self.cooldown = max(self.cooldown, now + retry_after)
            self.successes = 0
            self.pressure = self.paced_pressure = False
            self.epoch_started = self.cooldown
            if generation != self.generation:
                return
            self.generation += 1
            self.cap = max(1, self.cap // 2)
            if self.mode == "adaptive":
                self.spacing = max(self.spacing * 2, self.latency / self.cap, .25)
        elif 200 <= status < 300 and generation == self.generation:
            self.latency = .8 * self.latency + .2 * elapsed
            self.successes += 1
            if self.successes >= max(8, 2 * self.cap) and now >= self.cooldown and now - self.epoch_started >= 1:
                # Recover the gate that currently constrains useful throughput.
                if self.paced_pressure and self.spacing > self.latency / self.cap:
                    self.spacing *= .8
                    if self.spacing < .01:
                        self.spacing = 0
                elif self.pressure:
                    self.cap = min(self.ceiling, self.cap + 1)
                elif self.paced_pressure and self.spacing > 0:
                    self.spacing *= .8
                self.peak_cap = max(self.peak_cap, self.cap)
                self.successes = 0
                self.pressure = self.paced_pressure = False
                self.epoch_started = now
        elif status >= 500 or status == 0:
            self.cooldown = max(self.cooldown, now + retry_after)
            self.successes = 0
            self.pressure = self.paced_pressure = False
            self.epoch_started = self.cooldown


def simulate(mode, scenario, seed, count=160):
    rng = random.Random(seed)
    durations = [rng.uniform(.5, 1.5) for _ in range(count)]
    if scenario == "variable_latency":
        durations = [rng.choice([.25, 1, 8, 15]) for _ in range(count)]
    if scenario == "slow_request_quota":
        durations = [.2] * count
    policy = Controller(mode)
    # Mirror the pipeline's bounded conversation pool, not an unbounded queue
    # of already-started conversations. Backoff retains a conversation slot,
    # but releases the separate HTTP admission slot.
    workers = policy.cap if mode.startswith("fixed") else policy.ceiling
    items = iter(range(count))
    waiting = [(0., item, 0) for item in [next(items, None) for _ in range(workers)] if item is not None]
    heapq.heapify(waiting)
    active = []
    now, serial, completed, failed, peak = 0., 0, 0, 0, 0
    statuses, successful_starts = Counter(), []
    while waiting or active:
        assert len(waiting) + len(active) <= workers
        while active and active[0][0] <= now + 1e-9:
            _, _, item, attempt, status, began, generation, retry_after = heapq.heappop(active)
            policy.finish(now, generation, status, now - began, retry_after)
            statuses[status] += 1
            if status == 200:
                completed += 1
            elif attempt < 5:
                delay = retry_after or (5 * 2 ** min(attempt, 4) if status == 429 else attempt + 1)
                heapq.heappush(waiting, (now + delay, item, attempt + 1))
            else:
                failed += 1
            if status == 200 or attempt == 5:
                replacement = next(items, None)
                if replacement is not None:
                    heapq.heappush(waiting, (now, replacement, 0))
        while waiting and len(active) < policy.cap and max(waiting[0][0], policy.next_start, policy.cooldown) <= now + 1e-9:
            _, item, attempt = heapq.heappop(waiting)
            generation = policy.admit(now, len(active))
            cap = 4 if scenario == "concurrency_4" else 100
            if scenario == "capacity_drop":
                cap = 8 if now < 12 else 2
            in_provider = sum(event[4] == 200 for event in active)
            successful_starts = [start for start in successful_starts if now - start < 1 - 1e-9]
            status, retry_after = 200, 0.
            if in_provider >= cap:
                status, retry_after = 429, 1.
            if scenario == "requests_3_per_second" and len(successful_starts) >= 3:
                status, retry_after = 429, max(.01, successful_starts[0] + 1 - now)
            if scenario == "slow_request_quota" and successful_starts:
                status, retry_after = 429, max(.01, successful_starts[0] + 1 - now)
            if scenario == "transient_503" and item % 17 == 0 and attempt == 0:
                status = 503
            duration = durations[item] if status == 200 else .05
            if status == 200:
                successful_starts.append(now)
            serial += 1
            heapq.heappush(active, (now + duration, serial, item, attempt, status, now, generation, retry_after))
            peak = max(peak, len(active))
        events = [active[0][0]] if active else []
        ready_work = bool(waiting) and waiting[0][0] <= now
        policy.pressure |= ready_work and len(active) >= policy.cap
        policy.paced_pressure |= ready_work and len(active) < policy.cap and policy.next_start > now
        if waiting and len(active) < policy.cap:
            events.append(max(waiting[0][0], policy.next_start, policy.cooldown))
        if events:
            now = max(now, min(events))
    return dict(mode=mode, scenario=scenario, seed=seed, completed=completed, failed=failed,
                wall_seconds=now, requests=sum(statuses.values()), statuses=dict(statuses),
                peak_inflight=peak, final_cap=policy.cap, final_spacing_s=policy.spacing)


def sample_trace(path):
    pools = {"importance_comparison": [], "novelty_comparison": []}
    for line in path.read_text().splitlines():
        record = json.loads(line)
        if record.get("event") != "request" or record.get("conversation_turn") != 0 or record.get("provider_attempt") != 0:
            continue
        if record.get("workflow") not in pools:
            continue
        request = record["request"]
        text = request["messages"][0]["content"][0]["Text"]
        task = json.loads(text)
        if "candidates" not in task["comparisons"][0]:
            continue
        pools[record["workflow"]].append(dict(
            workflow=record["workflow"], task=task, endpoint=record["endpoint"],
            payload=dict(model=record["model"], temperature=0, max_tokens=16384,
                         thinking={"type": "disabled"}, response_format={"type": "json_object"},
                         messages=[dict(role="system", content=request["system"]),
                                   dict(role="user", content=text)])))
    result = []
    for pool in pools.values():
        if len(pool) < 12:
            raise ValueError("Need 12 successful-format initial requests per metric")
        result.extend(pool[round(i * (len(pool) - 1) / 11)] for i in range(12))
    random.Random(91).shuffle(result)
    return result


def validate(spec, body):
    try:
        content = body["choices"][0]["message"]["content"].strip()
        if content.startswith("```json") and content.endswith("```"):
            content = content[7:-3].strip()
        decisions = json.loads(content)["comparisons"]
        groups = {group["comparison_id"]: group["candidates"] for group in spec["task"]["comparisons"]}
        assert len(decisions) == len(groups)
        seen = set()
        for decision in decisions:
            cid = decision["comparison_id"]
            assert cid not in seen
            seen.add(cid)
            assert decision["most"] in groups[cid] and decision["least"] in groups[cid]
            assert decision["most"] != decision["least"]
        return True
    except (KeyError, TypeError, ValueError, AssertionError):
        return False


async def live_cell(mode, sample, key, directory):
    policy = Controller(mode)
    records, running = [], {}
    todo = iter(enumerate(sample))
    next_item = next(todo, None)
    started = time.monotonic()
    async with httpx.AsyncClient(timeout=90) as client:
        async def one(index, spec, generation):
            began = time.monotonic()
            try:
                response = await client.post(spec["endpoint"].rstrip("/") + "/chat/completions",
                                             json=spec["payload"], headers={"Authorization": f"Bearer {key}"})
                try:
                    body = response.json()
                except ValueError:
                    body = {"raw_text": response.text}
                status = response.status_code
                advice = response.headers.get("retry-after", "0")
                retry_after = float(advice) if advice.isdecimal() else 5.
            except httpx.HTTPError as error:
                body, status, retry_after = {"error_type": type(error).__name__}, 0, 0
            elapsed = time.monotonic() - began
            policy.finish(time.monotonic(), generation, status, elapsed, retry_after)
            record = dict(index=index, workflow=spec["workflow"], status=status, elapsed_s=elapsed,
                          valid=status == 200 and validate(spec, body), response=body,
                          cap=policy.cap, spacing_s=policy.spacing)
            (directory / f"response-{index:03}.json").write_text(json.dumps(record, ensure_ascii=False, indent=2) + "\n")
            records.append(record)
            return record

        while next_item or running:
            now = time.monotonic()
            while next_item and len(running) < policy.cap and max(policy.next_start, policy.cooldown) <= now:
                index, spec = next_item
                generation = policy.admit(now, len(running))
                task = asyncio.create_task(one(index, spec, generation))
                running[task] = index
                next_item = next(todo, None)
                now = time.monotonic()
            wake = None
            policy.pressure |= next_item is not None and len(running) >= policy.cap
            policy.paced_pressure |= next_item is not None and len(running) < policy.cap and policy.next_start > now
            if next_item and len(running) < policy.cap:
                wake = max(0, max(policy.next_start, policy.cooldown) - now)
            if running:
                done, _ = await asyncio.wait(running, timeout=wake, return_when=asyncio.FIRST_COMPLETED)
                for task in done:
                    running.pop(task)
                    result = task.result()
                    # A bounded probe must not keep loading an endpoint in distress.
                    if result["status"] != 200:
                        next_item = None
            elif wake:
                await asyncio.sleep(wake)
    return dict(mode=mode, wall_seconds=time.monotonic() - started, requests=len(records),
                valid=sum(record["valid"] for record in records), statuses=dict(Counter(r["status"] for r in records)),
                median_latency_s=statistics.median(r["elapsed_s"] for r in records),
                peak_cap=policy.peak_cap, final_cap=policy.cap, final_spacing_s=policy.spacing)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    parser.add_argument("--live-trace", type=Path)
    parser.add_argument("--api-key-stdin", action="store_true")
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    scenarios = ["unlimited", "concurrency_4", "requests_3_per_second", "slow_request_quota",
                 "variable_latency", "capacity_drop", "transient_503"]
    simulations = [simulate(mode, scenario, seed) for scenario in scenarios for seed in range(3)
                   for mode in ["fixed2", "fixed8", "aimd", "adaptive"]]
    (args.output / "simulation.json").write_text(json.dumps(simulations, indent=2) + "\n")
    for scenario in scenarios:
        for mode in ["fixed2", "fixed8", "aimd", "adaptive"]:
            rows = [r for r in simulations if r["scenario"] == scenario and r["mode"] == mode]
            print(scenario, mode, round(statistics.mean(r["wall_seconds"] for r in rows), 2),
                  "429s", round(statistics.mean(r["statuses"].get(429, 0) for r in rows), 1),
                  "failed", sum(r["failed"] for r in rows), flush=True)
    if args.live_trace:
        sample = sample_trace(args.live_trace)
        (args.output / "sample.json").write_text(json.dumps(sample, indent=2, ensure_ascii=False) + "\n")
        key = sys.stdin.readline().strip() if args.api_key_stdin else os.environ["BEYOND_SLIDES_API_KEY"]
        summaries = []
        # Reverse order on repeat to reduce simple time-of-run confounding.
        for round_index, modes in enumerate([["fixed2", "fixed4", "fixed8", "adaptive"],
                                             ["adaptive", "fixed8", "fixed4", "fixed2"]]):
            for mode in modes:
                cell = args.output / f"round-{round_index + 1}-{mode}"
                cell.mkdir()
                summary = asyncio.run(live_cell(mode, sample, key, cell))
                summary["round"] = round_index + 1
                summaries.append(summary)
                (args.output / "live-summary.json").write_text(json.dumps(summaries, indent=2) + "\n")
                print(json.dumps(summary), flush=True)
                if summary["statuses"].get(200, 0) < len(sample):
                    raise SystemExit("Stopped live probe on provider failure; partial evidence preserved")


if __name__ == "__main__":
    main()
