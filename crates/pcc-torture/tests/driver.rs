//! Runs the real binary end to end on a tiny corpus written at test time.

use std::{fs, path::PathBuf, process::Command};

const HEADER: &str = "file_id,sha256,bytes,faces,license,defects";

/// A unit tetrahedron as binary STL.
fn tetrahedron_stl() -> Vec<u8> {
    let (a, b, c, d) = (
        [0f32, 0., 0.],
        [1f32, 0., 0.],
        [0f32, 1., 0.],
        [0f32, 0., 1.],
    );
    let faces = [[a, c, b], [a, b, d], [b, c, d], [a, d, c]];
    let mut bytes = vec![0u8; 80];
    bytes.extend_from_slice(&(faces.len() as u32).to_le_bytes());
    for face in faces {
        bytes.extend_from_slice(&[0u8; 12]);
        for vertex in face {
            for component in vertex {
                bytes.extend_from_slice(&component.to_le_bytes());
            }
        }
        bytes.extend_from_slice(&[0u8; 2]);
    }
    bytes
}

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(name: &str, rows: &[&str]) -> Result<Self, std::io::Error> {
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
        if root.exists() {
            fs::remove_dir_all(&root)?;
        }
        fs::create_dir_all(root.join("corpus"))?;
        fs::write(root.join("corpus/1.stl"), tetrahedron_stl())?;
        fs::write(root.join("corpus/2.stl"), b"not a mesh")?;
        let manifest = format!("# test\n{HEADER}\n{}\n", rows.join("\n"));
        fs::write(root.join("manifest.csv"), manifest)?;
        Ok(Self { root })
    }

    /// Runs the driver and returns `(success, records)`.
    fn run(&self, ops: &str) -> Result<(bool, Vec<serde_json::Value>), std::io::Error> {
        self.run_timed(ops, "30", "10")
    }

    fn run_timed(
        &self,
        ops: &str,
        timeout: &str,
        grace: &str,
    ) -> Result<(bool, Vec<serde_json::Value>), std::io::Error> {
        let out = self.root.join("out.jsonl");
        let status = Command::new(env!("CARGO_BIN_EXE_pcc-torture"))
            .args([
                "run",
                "--jobs",
                "2",
                "--timeout",
                timeout,
                "--grace",
                grace,
                "--ops",
                ops,
            ])
            .arg("--manifest")
            .arg(self.root.join("manifest.csv"))
            .arg("--corpus")
            .arg(self.root.join("corpus"))
            .arg("--out")
            .arg(&out)
            .output()?;
        let records = fs::read_to_string(&out)?
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();
        Ok((status.status.success(), records))
    }
}

#[test]
fn a_healthy_corpus_passes_and_every_run_is_recorded() -> Result<(), std::io::Error> {
    let fixture = Fixture::new("healthy", &["1,x,0,4,CC,clean"])?;
    let (success, records) = fixture.run("clip,triangulate,normal")?;
    assert!(success);
    assert_eq!(records.len(), 3);
    for record in &records {
        assert_eq!(record["mesh"], "1");
        assert_eq!(record["status"], "ok", "{record}");
        assert_eq!(record["input"]["closed"], true);
        assert_eq!(record["input"]["simple_solid"], true);
        assert!(record["hash"].as_str().is_some_and(|hash| hash.len() == 64));
    }
    Ok(())
}

#[test]
fn unparsable_meshes_are_rejected_and_missing_ones_fail_the_run() -> Result<(), std::io::Error> {
    let garbage = Fixture::new("garbage", &["2,x,0,4,CC,clean"])?;
    let (success, records) = garbage.run("clip")?;
    assert!(success, "a clean rejection is not a defect");
    assert_eq!(records[0]["status"], "rejected", "{}", records[0]);

    let missing = Fixture::new("missing", &["3,x,0,4,CC,clean"])?;
    let (success, records) = missing.run("clip")?;
    assert!(!success, "a mesh that cannot be read must fail the run");
    assert_eq!(records[0]["status"], "crash", "{}", records[0]);
    Ok(())
}

#[test]
fn unknown_operators_and_bad_arguments_do_not_pass_silently() -> Result<(), std::io::Error> {
    let fixture = Fixture::new("unknown", &["1,x,0,4,CC,clean"])?;
    let (_, records) = fixture.run("no_such_operator")?;
    assert_eq!(records[0]["status"], "rejected", "{}", records[0]);
    let usage = Command::new(env!("CARGO_BIN_EXE_pcc-torture"))
        .arg("nonsense")
        .output()?;
    assert!(!usage.status.success());
    Ok(())
}

/// One deliberately failing operator, with how the supervisor must classify it.
fn assert_fault(operator: &str, expected: &str, run_fails: bool) -> Result<(), std::io::Error> {
    let name = operator.replace(':', "_");
    let fixture = Fixture::new(&name, &["1,x,0,4,CC,clean"])?;
    let (success, records) = fixture.run_timed(operator, "1", "1")?;
    assert_eq!(records.len(), 1, "{operator}");
    assert_eq!(records[0]["status"], expected, "{operator}: {}", records[0]);
    assert_eq!(success, !run_fails, "{operator}: wrong exit status");
    Ok(())
}

#[test]
fn the_supervisor_detects_panics_crashes_and_hangs() -> Result<(), std::io::Error> {
    assert_fault("fault:panic", "panic", true)?;
    assert_fault("fault:abort", "crash", true)?;
    // Ignores cancellation, so it can only end by being killed after the grace period.
    assert_fault("fault:hang", "hang", true)?;
    // Honours cancellation at the soft limit: a limit reached, not a defect.
    assert_fault("fault:slow", "timed_out", false)
}
