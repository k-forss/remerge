#!/usr/bin/env python3

from __future__ import annotations

import argparse
import json
import sys
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Dict
import xml.etree.ElementTree as ET


@dataclass(frozen=True)
class Regression:
    name: str
    baseline: float
    current: float

    @property
    def ratio(self) -> float:
        if self.baseline == 0:
            return float("inf")
        return self.current / self.baseline


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Compare nextest JUnit timings against a checked-in baseline."
    )
    parser.add_argument("--junit", required=True, type=Path, help="Path to JUnit XML")
    parser.add_argument(
        "--baseline",
        required=True,
        type=Path,
        help="Path to the checked-in duration baseline JSON",
    )
    parser.add_argument(
        "--threshold",
        type=float,
        default=1.25,
        help="Maximum allowed slowdown multiplier before failing (default: 1.25)",
    )
    parser.add_argument(
        "--minimum-duration",
        type=float,
        default=0.1,
        help=(
            "Absolute duration floor in seconds for very short baseline tests; "
            "tests with smaller baselines are allowed up to at least this duration "
            "before failing (default: 0.1)"
        ),
    )
    parser.add_argument(
        "--minimum-regression",
        type=float,
        default=0.5,
        help=(
            "Minimum absolute slowdown in seconds required before failing a test "
            "as a regression (default: 0.5)"
        ),
    )
    parser.add_argument(
        "--report",
        type=Path,
        help="Optional path to write a human-readable report",
    )
    parser.add_argument(
        "--write-baseline",
        action="store_true",
        help="Write the current JUnit timings into the baseline file instead of comparing",
    )
    return parser.parse_args()


def testcase_name(testcase: ET.Element) -> str:
    name = testcase.attrib.get("name", "unnamed-test")
    classname = testcase.attrib.get("classname", "")
    if classname and not name.startswith(f"{classname}::"):
        return f"{classname}::{name}"
    return name


def load_junit_timings(path: Path) -> Dict[str, float]:
    root = ET.parse(path).getroot()
    timings: Dict[str, float] = {}
    for testcase in root.iter("testcase"):
        name = testcase_name(testcase)
        try:
            timings[name] = float(testcase.attrib.get("time", "0"))
        except ValueError:
            timings[name] = 0.0
    return dict(sorted(timings.items()))


def load_baseline(path: Path) -> Dict[str, float]:
    data = json.loads(path.read_text())
    tests = data.get("tests")
    if not isinstance(tests, dict):
        raise ValueError(f"baseline file {path} does not contain a tests object")
    return {str(name): float(duration) for name, duration in tests.items()}


def write_baseline(path: Path, timings: Dict[str, float], threshold: float) -> None:
    payload = {
        "schema": 1,
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "threshold_multiplier": threshold,
        "tests": timings,
    }
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n")


def compare_timings(
    baseline: Dict[str, float],
    current: Dict[str, float],
    threshold: float,
    minimum_duration: float,
    minimum_regression: float,
) -> tuple[list[Regression], list[str], list[str]]:
    regressions: list[Regression] = []
    new_tests: list[str] = []

    for name, current_duration in current.items():
        baseline_duration = baseline.get(name)
        if baseline_duration is None:
            new_tests.append(name)
            continue
        allowed_duration = baseline_duration * threshold
        if baseline_duration < minimum_duration:
            allowed_duration = max(allowed_duration, minimum_duration)
        slowdown = current_duration - baseline_duration
        if (
            baseline_duration > 0
            and current_duration > allowed_duration
            and slowdown >= minimum_regression
        ):
            regressions.append(
                Regression(name=name, baseline=baseline_duration, current=current_duration)
            )

    missing_tests = sorted(name for name in baseline.keys() if name not in current)
    regressions.sort(key=lambda item: item.ratio, reverse=True)
    return regressions, sorted(new_tests), missing_tests


def build_report(
    regressions: list[Regression],
    new_tests: list[str],
    missing_tests: list[str],
    current_count: int,
    baseline_count: int,
    threshold: float,
    minimum_duration: float,
    minimum_regression: float,
) -> str:
    lines = [
        f"Compared {current_count} timed tests against {baseline_count} baseline entries.",
        f"Hard-fail threshold: {threshold:.2f}x baseline ({(threshold - 1.0) * 100:.0f}% slower).",
        (
            f"Short-test floor: baseline entries below {minimum_duration * 1000:.0f}ms "
            f"are allowed up to {minimum_duration * 1000:.0f}ms before failing."
        ),
        f"Minimum absolute slowdown: {minimum_regression:.3f}s.",
    ]

    if regressions:
        lines.append("")
        lines.append("Duration regressions:")
        for regression in regressions:
            lines.append(
                "  - "
                f"{regression.name}: {regression.current:.3f}s current vs {regression.baseline:.3f}s baseline "
                f"({regression.ratio:.2f}x)"
            )
    else:
        lines.append("")
        lines.append("No tracked test exceeded the configured slowdown threshold.")

    if new_tests:
        lines.append("")
        lines.append("New tests without baseline entries:")
        for name in new_tests:
            lines.append(f"  - {name}")

    if missing_tests:
        lines.append("")
        lines.append("Baseline entries not exercised in this run:")
        for name in missing_tests:
            lines.append(f"  - {name}")

    return "\n".join(lines) + "\n"


def main() -> int:
    args = parse_args()
    current = load_junit_timings(args.junit)

    if args.write_baseline:
        write_baseline(args.baseline, current, args.threshold)
        message = (
            f"Wrote {len(current)} test durations to baseline {args.baseline} "
            f"with threshold {args.threshold:.2f}x.\n"
        )
        if args.report:
            args.report.parent.mkdir(parents=True, exist_ok=True)
            args.report.write_text(message)
        sys.stdout.write(message)
        return 0

    baseline = load_baseline(args.baseline)
    regressions, new_tests, missing_tests = compare_timings(
        baseline,
        current,
        args.threshold,
        args.minimum_duration,
        args.minimum_regression,
    )
    report = build_report(
        regressions,
        new_tests,
        missing_tests,
        current_count=len(current),
        baseline_count=len(baseline),
        threshold=args.threshold,
        minimum_duration=args.minimum_duration,
        minimum_regression=args.minimum_regression,
    )

    if args.report:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(report)

    sys.stdout.write(report)
    return 1 if regressions else 0


if __name__ == "__main__":
    raise SystemExit(main())