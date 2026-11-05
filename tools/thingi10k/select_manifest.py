#!/usr/bin/env python3
"""Selects the Thingi10K torture subset and writes tests/corpus/thingi10k-200.csv.

Selection is deterministic: the same dataset revision always yields the same manifest. Only IDs,
hashes, licenses, and defect labels are written. Meshes are never committed, because each one has
its own license. See tests/corpus/README.md.

Usage: select_manifest.py <directory-for-downloaded-metadata>
"""
import csv
import hashlib
import json
import pathlib
import re
import sys
import urllib.request

REPOSITORY = "Thingi10K/Thingi10K"
REVISION = "2d5d3b2f3cd3711028ad75b12788c13b25559ec6"
OUTPUT = pathlib.Path(__file__).resolve().parents[2] / "tests" / "corpus" / "thingi10k-200.csv"
TOTAL = 200
MAX_FACES = 150_000
# Licenses that allow commercial use and derivative works, so that the harness may freely transform
# the models. Non-commercial and no-derivatives models are excluded even for local testing.
ALLOWED_LICENSES = {
    "Creative Commons - Attribution",
    "Creative Commons - Attribution - Share Alike",
    "Creative Commons - Public Domain Dedication",
    "Public Domain",
    "BSD License",
}


def number(row, key):
    return float(row[key])


# Name, membership test, severity (higher is nastier), and how many models to take. "clean" is a
# control group: it separates operator bugs from legitimate rejection of broken input.
STRATA = [
    ("edge_nonmanifold", lambda r: number(r, "edge_manifold") == 0, lambda r: number(r, "num_faces"), 13),
    ("vertex_nonmanifold", lambda r: number(r, "vertex_manifold") == 0 and number(r, "edge_manifold") == 1, lambda r: number(r, "num_faces"), 13),
    ("open", lambda r: number(r, "num_boundary_edges") > 0, lambda r: number(r, "num_boundary_edges"), 13),
    ("nonoriented", lambda r: number(r, "oriented") == 0 and number(r, "edge_manifold") == 1, lambda r: number(r, "num_faces"), 13),
    ("self_intersecting", lambda r: number(r, "num_self_intersections") > 0, lambda r: number(r, "num_self_intersections"), 13),
    ("coplanar_overlap", lambda r: number(r, "num_coplanar_intersecting_faces") > 0, lambda r: number(r, "num_coplanar_intersecting_faces"), 13),
    ("degenerate_geometric", lambda r: number(r, "num_geometrical_degenerated_faces") > 0, lambda r: number(r, "num_geometrical_degenerated_faces"), 13),
    ("degenerate_combinatorial", lambda r: number(r, "num_combinatorial_degenerated_faces") > 0, lambda r: number(r, "num_combinatorial_degenerated_faces"), 13),
    ("duplicated_faces", lambda r: number(r, "num_duplicated_faces") > 0, lambda r: number(r, "num_duplicated_faces"), 13),
    ("many_components", lambda r: number(r, "num_connected_components") > 1, lambda r: number(r, "num_connected_components"), 13),
    ("high_genus", lambda r: number(r, "euler_characteristic") < -20, lambda r: -number(r, "euler_characteristic"), 13),
    ("extreme_aspect", lambda r: number(r, "max_aspect_ratio") > 1000, lambda r: number(r, "max_aspect_ratio"), 13),
    ("non_pwn", lambda r: number(r, "PWN") == 0, lambda r: number(r, "num_faces"), 13),
    ("large", lambda r: number(r, "num_faces") > 50_000, lambda r: number(r, "num_faces"), 11),
    ("clean", lambda r: (
        number(r, "edge_manifold") == 1 and number(r, "vertex_manifold") == 1 and number(r, "oriented") == 1
        and number(r, "num_boundary_edges") == 0 and number(r, "num_self_intersections") == 0
        and number(r, "num_geometrical_degenerated_faces") == 0 and number(r, "num_duplicated_faces") == 0
        and number(r, "num_connected_components") == 1
    ), lambda r: number(r, "num_faces"), 20),
]


def stable_order(file_id):
    return hashlib.sha256(file_id.encode()).hexdigest()


def fetch_tree():
    """Returns {file_id: sha256} for every raw mesh at the pinned revision, following pagination."""
    url = f"https://huggingface.co/api/datasets/{REPOSITORY}/tree/{REVISION}/raw_meshes?limit=1000"
    hashes = {}
    while url:
        with urllib.request.urlopen(url, timeout=60) as response:
            for entry in json.load(response):
                name = pathlib.PurePosixPath(entry["path"]).name
                if name.lower().endswith(".stl") and "lfs" in entry:
                    hashes[name[:-4]] = (entry["lfs"]["oid"], entry["size"])
            match = re.search(r'<([^>]+)>;\s*rel="next"', response.headers.get("Link") or "")
            url = match.group(1) if match else None
    return hashes


def main():
    metadata = pathlib.Path(sys.argv[1])
    with open(metadata / "input_summary.csv", newline="", encoding="utf8") as f:
        licenses = {row["ID"]: row["License"] for row in csv.DictReader(f)}
    with open(metadata / "geometry_data.csv", newline="", encoding="utf8") as f:
        geometry = list(csv.DictReader(f))
    hashes = fetch_tree()
    print(f"{len(geometry)} geometry rows, {len(hashes)} raw STL files")

    eligible = [
        row for row in geometry
        if licenses.get(row["file_id"]) in ALLOWED_LICENSES
        and row["file_id"] in hashes
        and 4 <= number(row, "num_faces") <= MAX_FACES
    ]
    print(f"{len(eligible)} eligible after license, format, and size filters")

    chosen = {}
    pools = []
    for name, member, severity, quota in STRATA:
        pool = [row for row in eligible if member(row)]
        worst = sorted(pool, key=lambda r: (-severity(r), stable_order(r["file_id"])))
        spread = sorted(pool, key=lambda r: stable_order(r["file_id"]))
        print(f"  {name}: {len(pool)} eligible")
        pools.append((name, member, worst, spread, quota))

    for name, member, worst, spread, quota in pools:
        # Half the most severe examples, half an unbiased sample of the stratum.
        taken = 0
        for source, limit in ((worst, quota // 2), (spread, quota)):
            for row in source:
                if taken >= limit:
                    break
                if row["file_id"] not in chosen:
                    chosen[row["file_id"]] = row
                    taken += 1

    # Top up deterministically if overlaps left the manifest short.
    for row in sorted(eligible, key=lambda r: stable_order(r["file_id"])):
        if len(chosen) >= TOTAL:
            break
        chosen.setdefault(row["file_id"], row)
    if len(chosen) > TOTAL:
        raise SystemExit(f"quotas selected {len(chosen)} models, more than {TOTAL}")

    OUTPUT.parent.mkdir(parents=True, exist_ok=True)
    with open(OUTPUT, "w", newline="\n", encoding="utf8") as f:
        f.write(f"# Thingi10K torture subset. Dataset revision: {REVISION}\n")
        f.write("file_id,sha256,bytes,faces,license,defects\n")
        for file_id in sorted(chosen, key=int):
            row = chosen[file_id]
            labels = "+".join(name for name, member, _, _, _ in pools if member(row)) or "none"
            sha, size = hashes[file_id]
            if "," in licenses[file_id]:
                raise SystemExit(f"license of {file_id} contains a comma")
            f.write(f"{file_id},{sha},{size},{int(number(row, 'num_faces'))},{licenses[file_id]},{labels}\n")
    print(f"wrote {len(chosen)} models to {OUTPUT}")


if __name__ == "__main__":
    main()
