#!/usr/bin/env python3
"""Downloads the meshes in a manifest into corpus/thingi10k/, verifying every SHA-256.

Usage: fetch.py [manifest] [destination]
"""
import csv
import hashlib
import pathlib
import sys
import time
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parents[2]
BASE = "https://huggingface.co/datasets/Thingi10K/Thingi10K/resolve"


def sha256_of(path):
    digest = hashlib.sha256()
    with open(path, "rb") as f:
        while chunk := f.read(1 << 20):
            digest.update(chunk)
    return digest.hexdigest()


def main():
    manifest = pathlib.Path(sys.argv[1]) if len(sys.argv) > 1 else ROOT / "tests/corpus/thingi10k-200.csv"
    destination = pathlib.Path(sys.argv[2]) if len(sys.argv) > 2 else ROOT / "corpus/thingi10k"
    destination.mkdir(parents=True, exist_ok=True)
    with open(manifest, encoding="utf8", newline="") as f:
        header = f.readline()
        revision = header.rsplit(" ", 1)[-1].strip()
        rows = list(csv.DictReader(f))
    fetched = present = 0
    for index, row in enumerate(rows, start=1):
        target = destination / f"{row['file_id']}.stl"
        if target.exists() and sha256_of(target) == row["sha256"]:
            present += 1
            continue
        url = f"{BASE}/{revision}/raw_meshes/{row['file_id']}.stl"
        partial = target.with_suffix(".partial")
        for attempt in range(4):
            try:
                urllib.request.urlretrieve(url, partial)
                break
            except OSError as error:
                if attempt == 3:
                    raise
                print(f"retrying {row['file_id']}: {error}", flush=True)
                time.sleep(2 ** attempt)
        actual = sha256_of(partial)
        if actual != row["sha256"]:
            partial.unlink()
            raise SystemExit(f"{row['file_id']}: expected {row['sha256']}, downloaded {actual}")
        partial.replace(target)
        fetched += 1
        if index % 20 == 0:
            print(f"{index}/{len(rows)}", flush=True)
    print(f"{fetched} downloaded, {present} already present, all hashes verified")


if __name__ == "__main__":
    main()
