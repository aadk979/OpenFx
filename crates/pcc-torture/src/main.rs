//! Command-line driver. `run` schedules one child process per (mesh, operator) pair; `one` is the
//! child. See `tests/corpus/README.md`.

use pcc_io::from_stl_bytes;
use pcc_sop::SopContext;
use pcc_torture::{
    MEMORY_LIMIT, OPERATORS, Outcome, Status, input_facts, is_simple_solid, parse_manifest,
    prepare, run_operator,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, VecDeque},
    env, fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, ExitCode, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

const MAX_TRIANGLES: usize = 2_000_000;
const USAGE: &str = "pcc-torture run [--manifest PATH] [--corpus DIR] [--out PATH] [--jobs N]
                 [--timeout SECONDS] [--grace SECONDS] [--ops a,b] [--ids a,b] [--limit N]
pcc-torture one --mesh PATH --op NAME --timeout SECONDS [--defects a+b]";

struct Options {
    manifest: PathBuf,
    corpus: PathBuf,
    out: PathBuf,
    jobs: usize,
    timeout: u64,
    grace: u64,
    operators: Vec<String>,
    ids: Vec<String>,
    limit: usize,
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn list(value: Option<String>) -> Vec<String> {
    value.map_or_else(Vec::new, |v| v.split(',').map(str::to_owned).collect())
}

fn parse_options(args: &[String]) -> Result<Options, String> {
    let number = |name: &str, default: u64| -> Result<u64, String> {
        flag(args, name).map_or(Ok(default), |v| {
            v.parse().map_err(|_| format!("{name} needs a number"))
        })
    };
    let parallelism = thread::available_parallelism().map_or(2, |n| n.get());
    let operators = list(flag(args, "--ops"));
    Ok(Options {
        manifest: flag(args, "--manifest")
            .unwrap_or_else(|| "tests/corpus/thingi10k-200.csv".to_owned())
            .into(),
        corpus: flag(args, "--corpus")
            .unwrap_or_else(|| "corpus/thingi10k".to_owned())
            .into(),
        out: flag(args, "--out")
            .unwrap_or_else(|| "reports/thingi10k.jsonl".to_owned())
            .into(),
        jobs: number("--jobs", (parallelism / 2).max(1) as u64)? as usize,
        timeout: number("--timeout", 30)?,
        grace: number("--grace", 30)?,
        operators: if operators.is_empty() {
            OPERATORS.iter().map(|s| (*s).to_owned()).collect()
        } else {
            operators
        },
        ids: list(flag(args, "--ids")),
        limit: number("--limit", u64::MAX)? as usize,
    })
}

/// Deliberate failures, reachable only by naming them, that prove the supervisor can detect each
/// outcome. A torture run that has never seen a panic means little if a panic would go unnoticed.
fn inject_fault(operator: &str, cancelled: &AtomicBool) -> Option<Outcome> {
    match operator {
        "fault:panic" => {
            let empty: Vec<u8> = Vec::new();
            let index = empty.len();
            // Indexing out of bounds panics without the macros that the audit forbids.
            println!("{}", empty[index]);
            None
        }
        "fault:abort" => std::process::abort(),
        "fault:hang" => loop {
            thread::sleep(Duration::from_millis(50));
        },
        "fault:slow" => {
            while !cancelled.load(Ordering::Acquire) {
                thread::sleep(Duration::from_millis(5));
            }
            Some(Outcome {
                status: Status::TimedOut,
                detail: "Cancelled".to_owned(),
                out_points: 0,
                out_prims: 0,
                hash: String::new(),
            })
        }
        _ => None,
    }
}

/// Child process: runs one operator twice and prints one JSON line.
fn child(args: &[String]) -> ExitCode {
    let (Some(mesh), Some(operator), Some(timeout)) = (
        flag(args, "--mesh"),
        flag(args, "--op"),
        flag(args, "--timeout").and_then(|v| v.parse::<u64>().ok()),
    ) else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let cancelled = Arc::new(AtomicBool::new(false));
    let watchdog = Arc::clone(&cancelled);
    thread::spawn(move || {
        thread::sleep(Duration::from_secs(timeout));
        watchdog.store(true, Ordering::Release);
    });
    let started = Instant::now();
    let report = |facts: Value, found: &Outcome, millis: u128, cancel_ms: Option<u128>| {
        let line = json!({
            "status": found.status.as_str(),
            "detail": found.detail,
            "millis": millis,
            "cancel_observed_ms": cancel_ms,
            "out_points": found.out_points,
            "out_prims": found.out_prims,
            "hash": found.hash,
            "input": facts,
        });
        println!("{line}");
        ExitCode::SUCCESS
    };
    let empty = json!({});
    let failure = |status: Status, detail: String| Outcome {
        status,
        detail,
        out_points: 0,
        out_prims: 0,
        hash: String::new(),
    };
    let bytes = match fs::read(&mesh) {
        Ok(bytes) => bytes,
        Err(error) => {
            let found = failure(Status::Crash, format!("cannot read mesh: {error}"));
            return report(empty, &found, 0, None);
        }
    };
    let parsed = match from_stl_bytes(&bytes, MAX_TRIANGLES) {
        Ok(geo) => geo,
        Err(error) => {
            let found = failure(Status::Rejected, format!("stl: {error:?}"));
            return report(empty, &found, 0, None);
        }
    };
    let ctx = SopContext::new(MEMORY_LIMIT, &cancelled);
    let prepared = match prepare(&parsed, &ctx) {
        Ok(geo) => geo,
        Err(error) => {
            let found = failure(Status::Rejected, format!("prepare: {error:?}"));
            return report(empty, &found, 0, None);
        }
    };
    let mut facts = input_facts(&prepared);
    facts.simple_solid =
        facts.closed && flag(args, "--defects").is_some_and(|d| is_simple_solid(&d));
    let facts_json = json!({
        "points": facts.points,
        "prims": facts.prims,
        "closed": facts.closed,
        "manifold": facts.manifold,
        "simple_solid": facts.simple_solid,
    });
    let first_start = Instant::now();
    let mut found = match inject_fault(&operator, &cancelled) {
        Some(fault) => fault,
        None => run_operator(&operator, &prepared, facts, &cancelled),
    };
    let millis = first_start.elapsed().as_millis();
    let limit = u128::from(timeout) * 1000;
    let cancel_ms = (found.status == Status::TimedOut)
        .then(|| started.elapsed().as_millis().saturating_sub(limit));
    // A second run in the same process catches anything depending on hash-map iteration order.
    if matches!(
        found.status,
        Status::Ok | Status::OkEmpty | Status::Invariant
    ) && started.elapsed().as_millis() < limit / 2
    {
        let repeat = run_operator(&operator, &prepared, facts, &cancelled);
        if repeat.status != Status::TimedOut && repeat.hash != found.hash {
            found.status = Status::Nondeterministic;
            found.detail = format!("{} then {}", found.hash, repeat.hash);
        }
    }
    report(facts_json, &found, millis, cancel_ms)
}

/// Runs one child under the hard time limit and classifies how it ended.
fn supervise(exe: &Path, mesh: &Path, defects: &str, operator: &str, options: &Options) -> Value {
    let started = Instant::now();
    let spawned = Command::new(exe)
        .args(["one", "--op", operator, "--timeout"])
        .arg(options.timeout.to_string())
        .args(["--defects", defects, "--mesh"])
        .arg(mesh)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut process = match spawned {
        Ok(process) => process,
        Err(error) => return classified(Status::Crash, &format!("spawn failed: {error}"), 0),
    };
    let hard_limit = Duration::from_secs(options.timeout + options.grace);
    // Drain pipes on their own threads so that a chatty child can never block on a full pipe.
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        thread::spawn(move || {
            let mut text = String::new();
            if let Some(mut pipe) = pipe {
                let mut bytes = Vec::new();
                if pipe.read_to_end(&mut bytes).is_ok() {
                    text = String::from_utf8_lossy(&bytes).into_owned();
                }
            }
            text
        })
    };
    let stdout = drain(
        process
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let stderr = drain(
        process
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let exit = loop {
        match process.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if started.elapsed() > hard_limit => {
                // A failed kill means that the process already ended, which is fine here.
                let _ = process.kill();
                let _ = process.wait();
                break None;
            }
            Ok(None) => thread::sleep(Duration::from_millis(5)),
            Err(error) => {
                let millis = started.elapsed().as_millis();
                return classified(Status::Crash, &format!("wait failed: {error}"), millis);
            }
        }
    };
    let out = stdout.join().unwrap_or_default();
    let err = stderr.join().unwrap_or_default();
    let millis = started.elapsed().as_millis();
    let Some(exit) = exit else {
        return classified(
            Status::Hang,
            "ignored cancellation past the grace period",
            millis,
        );
    };
    if exit.success()
        && let Some(line) = out.lines().last()
        && let Ok(value) = serde_json::from_str::<Value>(line)
    {
        return value;
    }
    let tail: String = err.lines().rev().take(6).collect::<Vec<_>>().join(" | ");
    // Rust's panic runtime exits with 101; every other failure is a crash.
    let status = if exit.code() == Some(101) {
        Status::Panic
    } else {
        Status::Crash
    };
    classified(status, &format!("exit {:?}: {tail}", exit.code()), millis)
}

fn classified(status: Status, detail: &str, millis: u128) -> Value {
    json!({ "status": status.as_str(), "detail": detail, "millis": millis })
}

fn summarize(records: &[Value], operators: &[String]) -> (String, usize) {
    let mut counts: BTreeMap<&str, BTreeMap<Status, usize>> = BTreeMap::new();
    let mut slowest: BTreeMap<&str, u64> = BTreeMap::new();
    let mut total_millis: BTreeMap<&str, u64> = BTreeMap::new();
    let mut failures = Vec::new();
    for record in records {
        let operator = record["op"].as_str().unwrap_or("?");
        let status = record["status"]
            .as_str()
            .and_then(Status::parse)
            .unwrap_or(Status::Crash);
        *counts
            .entry(operator)
            .or_default()
            .entry(status)
            .or_insert(0) += 1;
        let millis = record["millis"].as_u64().unwrap_or(0);
        let slot = slowest.entry(operator).or_insert(0);
        *slot = (*slot).max(millis);
        *total_millis.entry(operator).or_insert(0) += millis;
        if status.is_failure() {
            failures.push(record);
        }
    }
    let mut text = String::from("| operator |");
    for status in Status::ALL {
        text.push_str(&format!(" {} |", status.as_str()));
    }
    text.push_str(" total s | max s |\n|---|");
    text.push_str(&"---|".repeat(Status::ALL.len() + 2));
    text.push('\n');
    for operator in operators {
        let row = counts.get(operator.as_str());
        text.push_str(&format!("| {operator} |"));
        for status in Status::ALL {
            let count = row.and_then(|r| r.get(&status)).copied().unwrap_or(0);
            text.push_str(&format!(" {count} |"));
        }
        text.push_str(&format!(
            " {:.1} | {:.1} |\n",
            total_millis.get(operator.as_str()).copied().unwrap_or(0) as f64 / 1000.0,
            slowest.get(operator.as_str()).copied().unwrap_or(0) as f64 / 1000.0,
        ));
    }
    text.push_str(&format!("\n{} failures\n", failures.len()));
    for failure in failures.iter().take(60) {
        let detail: String = failure["detail"]
            .as_str()
            .unwrap_or("")
            .chars()
            .take(240)
            .collect();
        text.push_str(&format!(
            "  {} {} [{}] {}\n",
            failure["mesh"].as_str().unwrap_or("?"),
            failure["op"].as_str().unwrap_or("?"),
            failure["status"].as_str().unwrap_or("?"),
            detail
        ));
    }
    (text, failures.len())
}

fn run(args: &[String]) -> Result<ExitCode, String> {
    let options = Arc::new(parse_options(args)?);
    let manifest = fs::read_to_string(&options.manifest)
        .map_err(|e| format!("{}: {e}", options.manifest.display()))?;
    let mut entries = parse_manifest(&manifest)?;
    if !options.ids.is_empty() {
        entries.retain(|e| options.ids.contains(&e.file_id));
    }
    entries.truncate(options.limit);
    let mut queue = VecDeque::new();
    for entry in &entries {
        for operator in &options.operators {
            queue.push_back((
                entry.file_id.clone(),
                entry.defects.clone(),
                operator.clone(),
            ));
        }
    }
    let total = queue.len();
    let queue = Arc::new(Mutex::new(queue));
    let records = Arc::new(Mutex::new(Vec::with_capacity(total)));
    if let Some(parent) = options.out.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let log = Arc::new(Mutex::new(
        fs::File::create(&options.out).map_err(|e| format!("{}: {e}", options.out.display()))?,
    ));
    let exe = env::current_exe().map_err(|e| e.to_string())?;
    let started = Instant::now();
    let mut workers = Vec::new();
    for _ in 0..options.jobs {
        let (queue, records, log, options, exe) = (
            Arc::clone(&queue),
            Arc::clone(&records),
            Arc::clone(&log),
            Arc::clone(&options),
            exe.clone(),
        );
        workers.push(thread::spawn(move || {
            loop {
                let next = queue.lock().ok().and_then(|mut q| q.pop_front());
                let Some((file_id, defects, operator)) = next else {
                    break;
                };
                let mesh = options.corpus.join(format!("{file_id}.stl"));
                let mut record = supervise(&exe, &mesh, &defects, &operator, &options);
                record["mesh"] = json!(file_id);
                record["op"] = json!(operator);
                if let Ok(mut file) = log.lock() {
                    // A lost log line must not end a long run; the summary still has the record.
                    let _ = writeln!(file, "{record}");
                }
                if let Ok(mut all) = records.lock() {
                    all.push(record);
                    if all.len().is_multiple_of(100) {
                        eprintln!(
                            "{}/{total} in {:.0}s",
                            all.len(),
                            started.elapsed().as_secs_f64()
                        );
                    }
                }
            }
        }));
    }
    for worker in workers {
        worker.join().map_err(|_| "worker thread died".to_owned())?;
    }
    let all = records.lock().map_err(|_| "poisoned records".to_owned())?;
    let (summary, failures) = summarize(&all, &options.operators);
    println!("{summary}");
    println!(
        "{} models, {total} runs, {:.0}s wall clock, details in {}",
        entries.len(),
        started.elapsed().as_secs_f64(),
        options.out.display()
    );
    Ok(if failures == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("one") => child(&args),
        Some("run") => match run(&args) {
            Ok(code) => code,
            Err(message) => {
                eprintln!("error: {message}");
                ExitCode::from(2)
            }
        },
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}
