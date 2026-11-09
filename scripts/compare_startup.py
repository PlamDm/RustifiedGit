#!/usr/bin/env python3
"""Compare time to a populated Files panel for optimized Go and Rust builds."""
import argparse
import json
from pathlib import Path
import statistics
import tempfile
import time

from terminal_smoke import Session, fixture, git


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--rust", type=Path, default=Path(__file__).resolve().parents[1] / "target/release/pd-lazygit")
    parser.add_argument("--go", type=Path, required=True)
    parser.add_argument("--files", type=int, default=5000)
    parser.add_argument("--runs", type=int, default=5)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="pd-startup-") as directory:
        directory = Path(directory)
        root = directory / "repo"
        root.mkdir()
        fixture(root, args.files)
        (root / "000-ready").mkdir()
        (root / "000-ready/marker.txt").write_text("ready\n")
        git(root, "add", "000-ready")
        git(root, "commit", "-q", "-m", "marker")
        # The graph layout starts with changed files; keep its readiness marker visible.
        (root / "000-ready/marker.txt").write_text("ready modified\n")
        configs = {}
        for language in ["go", "rust"]:
            config = directory / language
            config.mkdir()
            (config / "config.yml").write_text("disableStartupPopups: true\nnotARepository: quit\nupdate:\n  method: never\ngit:\n  autoFetch: false\n  autoRefresh: false\ngui:\n  showUnchangedFiles: true\n  showFileTree: true\n")
            configs[language] = config
        timings = {"go": [], "rust": []}
        for _ in range(args.runs):
            for name, binary in [("go", args.go), ("rust", args.rust)]:
                session = Session(binary.resolve(), root, configs[name])
                finished = False
                try:
                    session.until("000-ready", timeout=20)
                    timings[name].append(round((time.monotonic() - session.started) * 1000, 3))
                    session.finish()
                    finished = True
                finally:
                    if not finished:
                        session.close()
        result = {"fixture_tracked_files": args.files + 3, "fixture_dirty_files": 1, "runs": args.runs, "measurement": "process launch to dirty readiness marker received from populated Files pane at 140x40 (each application's default layout)", "rust_binary": str(args.rust.resolve()), "go_binary": str(args.go.resolve()), "results": {name: {"median_ms": statistics.median(values), "samples_ms": values} for name, values in timings.items()}}
        result["go_over_rust_median_ratio"] = round(result["results"]["go"]["median_ms"] / result["results"]["rust"]["median_ms"], 3)
        print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
