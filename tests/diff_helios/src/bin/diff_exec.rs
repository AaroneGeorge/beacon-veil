//! Executor pass: every differential case through the `lc_verify_v3` guest (`VerifyOnce`) in the RISC Zero
//! executor, into `evidence/diff/executor.json`. The report run (`diff_report`, the nextest test) then compares each
//! entry with V1.
//!
//!   cargo run -p diff_helios --features executor --bin diff_exec -- --elf <lc_verify_v3 user ELF or .bin>
//!       [--threads N (default 2)] [--only ID_PREFIX] [--out PATH]
//!
//! Results are appended to `<out>.partial.jsonl` as they finish, so an interrupted run resumes where it stopped
//! (entries are reused only for the same case id, input hash and image id). Each worker runs one `r0vm` at a time.

use std::{
    collections::BTreeMap,
    io::Write,
    path::PathBuf,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Instant,
};

use anyhow::{Context, Result, bail};
use diff_helios::{all_cases, exec::ExecV3, report};
use serde_json::{Value, json};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let arg = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let Some(elf) = arg("--elf").or_else(|| std::env::var("BV_V3_ELF").ok()) else {
        bail!("--elf <lc_verify_v3 user ELF or .bin> (or BV_V3_ELF)");
    };
    let threads: usize = arg("--threads").map_or(Ok(2), |t| t.parse())?;
    let out = arg("--out").map_or_else(|| report::evidence_dir().join("executor.json"), PathBuf::from);
    let only = arg("--only");
    let partial = out.with_extension("partial.jsonl");

    let guest = ExecV3::load(elf.as_ref())?;
    eprintln!(
        "lc_verify_v3: {} sha256 {} image {}",
        guest.elf_path, guest.elf_sha256, guest.image_id
    );
    let cases: Vec<_> = all_cases()?
        .into_iter()
        .filter(|c| only.as_deref().is_none_or(|p| c.id.starts_with(p)))
        .collect();

    // Resume: entries of earlier runs with the same id, input hash and image.
    let mut done: BTreeMap<String, Value> = BTreeMap::new();
    if let Ok(text) = std::fs::read_to_string(&partial) {
        for line in text.lines() {
            let Ok(v) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            if v["image_id"] == guest.image_id.as_str() {
                done.insert(v["id"].as_str().unwrap_or_default().to_string(), v);
            }
        }
    }
    let todo: Vec<usize> = (0..cases.len())
        .filter(|i| {
            done.get(&cases[*i].id)
                .is_none_or(|v| v["input_sha256"].as_str() != Some(&cases[*i].input_sha256()))
        })
        .collect();
    eprintln!(
        "{} cases, {} from an earlier partial run, {} to execute on {threads} workers",
        cases.len(),
        cases.len() - todo.len(),
        todo.len()
    );

    let file = Mutex::new(
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&partial)
            .with_context(|| partial.display().to_string())?,
    );
    let results = Mutex::new(done);
    let next = AtomicUsize::new(0);
    let t0 = Instant::now();
    std::thread::scope(|s| {
        for _ in 0..threads.max(1) {
            s.spawn(|| {
                loop {
                    let k = next.fetch_add(1, Ordering::Relaxed);
                    let Some(&i) = todo.get(k) else { break };
                    let c = &cases[i];
                    let r = guest.execute(&c.config, &c.witness, &c.update);
                    let v = json!({
                        "id": c.id,
                        "input_sha256": c.input_sha256(),
                        "image_id": guest.image_id,
                        "verdict": r.verdict.short(),
                        "digest": r.verdict.digest(),
                        "cycles": r.cycles,
                    });
                    {
                        let mut f = file.lock().expect("partial file");
                        let _ = writeln!(f, "{v}");
                        let _ = f.flush();
                    }
                    results.lock().expect("results").insert(c.id.clone(), v);
                    if (k + 1).is_multiple_of(25) || k + 1 == todo.len() {
                        eprintln!("{}/{} executed, {:.0}s", k + 1, todo.len(), t0.elapsed().as_secs_f64());
                    }
                }
            });
        }
    });

    let results = results.into_inner().expect("results");
    let entries: Vec<&Value> = cases.iter().filter_map(|c| results.get(&c.id)).collect();
    let doc = json!({
        "what": "Every differential case through the lc_verify_v3 guest (one VerifyOnce message, LEZ actor input framing) in the RISC Zero executor; no proof. Verdict: exit code 0 with attest.Apply in the journal (accept), or exit code = EsaError number (refusal). digest: see Verdict::digest.",
        "generated_at_utc": report::utc_now(),
        "executor": "risc0-zkvm 3.0.5 default_executor (external r0vm 3.0.5)",
        "elf": PathBuf::from(&guest.elf_path).file_name().map(|f| f.to_string_lossy().into_owned()),
        "elf_sha256": guest.elf_sha256,
        "image_id": guest.image_id,
        "cases": entries,
    });
    std::fs::write(&out, serde_json::to_string_pretty(&doc)? + "\n")?;
    if only.is_none() && entries.len() == cases.len() {
        let _ = std::fs::remove_file(&partial);
    }
    let panics = entries.iter().filter(|e| e["verdict"] == "panic").count();
    println!(
        "executor: {} of {} cases written to {} ({panics} executor errors) in {:.0}s",
        entries.len(),
        cases.len(),
        out.display(),
        t0.elapsed().as_secs_f64()
    );
    Ok(())
}
