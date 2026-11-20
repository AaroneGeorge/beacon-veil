//! `cost bench-bls --elf P --out DIR`: the BLS measurement of the public-path design (`make bench-bls`).
//!
//! Runs the `bls_bench` guest (`guests/bls_bench`) in the RISC Zero 3.0.5 executor, one execution per job:
//!
//! - **variants**: every real update of `fixtures/mainnet` (4 period updates, 1 finality update) and
//!   `fixtures/sepolia` (Gloas; 4 period updates, 1 finality update), each through the whole-update pipeline in four
//!   variants (zkcrypto or blst, hashed or cached committee) and through today's V2. Every result must equal V1's
//!   (`attest_core::lc::verify_and_apply`, native): accept, same pre-store, post-store, head and record. Any
//!   difference stops the run.
//! - **phases**: three updates (mainnet finality, mainnet handoff `update[1]`, Sepolia Gloas finality) with their
//!   participation bits rewritten to 0, 1 and 170 absent keys (the signature then fails at the final exponentiation;
//!   the phase costs are what is measured), zkcrypto and blst, hashed committee.
//!
//! Output: `DIR/phases.json` and `DIR/bench.md`, deterministic for a given guest image and fixtures (no times, no
//! paths outside the repo), so two runs are byte-identical.

use anyhow::{Context, Result, anyhow, bail, ensure};
use attest_core::{
    AccountId, EsaError, InstanceConfig,
    lc::{StoreWitness, init_store, verify_and_apply},
};
use bls_bench_guest::{BEGIN_PHASES, CommitteeMode, Job, JobOutput, Lib, Outcome, PHASES, WholeJob, witness_v3};
use risc0_zkvm::{ExecutorEnv, default_executor};
use serde_json::{Value, json};

use crate::{Guest, load};

/// LEZ rc3 public execution cap per transaction (gas = user cycles).
const CAP: u64 = 10_000_000;

fn exec(g: &Guest, job: &Job) -> Result<(u64, JobOutput)> {
    let input = borsh::to_vec(job)?;
    let env = ExecutorEnv::builder().write_slice(&input).build()?;
    let info = default_executor().execute(env, &g.program)?;
    let out: JobOutput = borsh::from_slice(&info.journal.bytes).context("journal")?;
    Ok((info.cycles(), out))
}

fn lib_name(l: Lib) -> &'static str {
    match l {
        Lib::Zkcrypto => "zkcrypto",
        Lib::Blst => "blst",
    }
}

fn mode_name(m: CommitteeMode) -> &'static str {
    match m {
        CommitteeMode::Hashed => "hashed",
        CommitteeMode::Cached => "cached",
    }
}

fn marks_json(marks: &[(String, u64)]) -> (Value, u64, u64) {
    let mut o = serde_json::Map::new();
    let mut begin = 0;
    let mut sum = 0;
    for (i, (k, v)) in marks.iter().enumerate() {
        o.insert(k.clone(), json!(v));
        sum += v;
        if i < BEGIN_PHASES {
            begin += v;
        }
    }
    (Value::Object(o), begin, sum)
}

fn code(r: &Result<Outcome, EsaError>) -> String {
    match r {
        Ok(_) => "accept".into(),
        Err(e) => format!("{e:?}"),
    }
}

struct Net {
    config: InstanceConfig,
    bootstrap: Vec<u8>,
    updates: Vec<(String, Vec<u8>)>,
}

fn net(name: &'static str) -> Result<Net> {
    let c =
        fixtures::real::load(name, vec![AccountId::new([0x1C; 32])], 0).with_context(|| format!("fixtures/{name}"))?;
    let mut updates: Vec<(String, Vec<u8>)> = c
        .updates
        .iter()
        .enumerate()
        .map(|(i, u)| (format!("update[{i}]"), u.clone()))
        .collect();
    updates.push((
        format!("finality_update_{}", c.finality_attested_slot),
        c.finality_ssz.clone(),
    ));
    Ok(Net {
        config: c.config,
        bootstrap: c.bootstrap_ssz,
        updates,
    })
}

fn whole(config: &InstanceConfig, w: &StoreWitness, u: &[u8], lib: Lib, mode: CommitteeMode) -> Result<Job> {
    let (witness, root, keys) = witness_v3(config, w, u).map_err(|e| anyhow!("witness_v3: {e:?}"))?;
    Ok(Job::Whole(WholeJob {
        lib,
        mode,
        config: config.clone(),
        witness,
        cached_root: root,
        update_ssz: u.to_vec(),
        keys,
    }))
}

pub fn bench_bls(args: &[String]) -> Result<()> {
    let arg = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let elf = arg("--elf").context("--elf PATH (guests/bls_bench/build.sh output)")?;
    let out_dir = arg("--out").context("--out DIR")?;
    let g = load(&elf)?;
    println!("bls_bench {} image {}", g.path, g.image_id);

    let mut variant_rows = Vec::new();
    let mut phase_rows = Vec::new();
    let mut v1_checks = 0usize;
    // (network, update label) whose store witness the phase cases use.
    let phase_cases = [
        ("mainnet", "finality_update_15637644"),
        ("mainnet", "update[1]"),
        ("sepolia", "finality_update_11561824"),
    ];
    let mut found_phase_cases = 0usize;

    for name in ["mainnet", "sepolia"] {
        let n = net(name)?;
        let (mut w, _) = init_store(&n.config, &n.bootstrap).map_err(|e| anyhow!("{name} init: {e:?}"))?;
        for (label, u) in &n.updates {
            let v1 = verify_and_apply(&n.config, &w, u).map_err(|e| anyhow!("{name} {label}: V1 refused: {e:?}"))?;
            let want = Outcome {
                pre_store: v1.pre_store,
                post_store: v1.post_store,
                head: v1.head.clone(),
                record: v1.record.clone(),
            };
            let kind = if label.starts_with("finality") {
                "finality"
            } else if v1.handoff {
                "handoff"
            } else {
                "period"
            };
            let mut jobs: Vec<(String, Job)> = Vec::new();
            for lib in [Lib::Zkcrypto, Lib::Blst] {
                for mode in [CommitteeMode::Hashed, CommitteeMode::Cached] {
                    jobs.push((
                        format!("{}+{}", lib_name(lib), mode_name(mode)),
                        whole(&n.config, &w, u, lib, mode)?,
                    ));
                }
            }
            jobs.push((
                "v2_today".into(),
                Job::V2 {
                    config: n.config.clone(),
                    witness: w.clone(),
                    update_ssz: u.clone(),
                },
            ));
            for (variant, job) in &jobs {
                let (session, out) = exec(&g, job)?;
                if out.result.as_ref() != Ok(&want) {
                    bail!(
                        "{name} {label} {variant}: result {} differs from V1 (accept)",
                        code(&out.result)
                    );
                }
                v1_checks += 1;
                let (marks, begin, sum) = marks_json(&out.marks);
                println!("{name} {label} ({kind}) {variant}: session {session} user cycles, marked {sum}");
                variant_rows.push(json!({
                    "network": name, "update": label, "kind": kind, "participation": v1.participation,
                    "variant": variant, "verdict_equals_v1": true, "session_user_cycles": session,
                    "marked_cycles": sum, "begin_cycles": if variant == "v2_today" { Value::Null } else { json!(begin) },
                    "phases": marks,
                }));
            }
            if phase_cases.iter().any(|(nn, l)| *nn == name && l == label) {
                found_phase_cases += 1;
                for absent in [0usize, 1, 170] {
                    let pu = bls_bench_guest::with_absent(&n.config, u, absent)
                        .map_err(|e| anyhow!("with_absent: {e:?}"))?;
                    let native = verify_and_apply(&n.config, &w, &pu).err();
                    for lib in [Lib::Zkcrypto, Lib::Blst] {
                        let job = whole(&n.config, &w, &pu, lib, CommitteeMode::Hashed)?;
                        let (session, out) = exec(&g, &job)?;
                        ensure!(
                            out.result.as_ref().err() == native.as_ref(),
                            "{name} {label} absent {absent} {}: {} but V1 says {native:?}",
                            lib_name(lib),
                            code(&out.result)
                        );
                        v1_checks += 1;
                        let (marks, begin, sum) = marks_json(&out.marks);
                        println!(
                            "{name} {label} absent={absent} {}: begin {begin}, marked {sum}",
                            lib_name(lib)
                        );
                        phase_rows.push(json!({
                            "network": name, "update": label, "absent_keys": absent, "lib": lib_name(lib),
                            "committee": "hashed", "verdict": code(&out.result), "verdict_equals_v1": true,
                            "session_user_cycles": session, "marked_cycles": sum, "begin_cycles": begin,
                            "phases": marks,
                        }));
                    }
                }
            }
            w = v1.new_witness();
        }
    }
    ensure!(found_phase_cases == phase_cases.len(), "phase fixtures not found");

    // Key figures.
    let get = |r: &Value, k: &str| r["phases"][k].as_u64().unwrap_or(0);
    let blst_phase = |k: &str| {
        phase_rows
            .iter()
            .filter(|r| r["lib"] == "blst")
            .map(|r| get(r, k))
            .max()
            .unwrap_or(0)
    };
    let miller = blst_phase("miller");
    let final_exp = blst_phase("final_exp");
    let begin_170 = phase_rows
        .iter()
        .filter(|r| r["lib"] == "blst" && r["absent_keys"] == 170)
        .map(|r| r["begin_cycles"].as_u64().unwrap_or(0))
        .max()
        .unwrap_or(0);
    let handoff_whole = variant_rows
        .iter()
        .filter(|r| r["network"] == "mainnet" && r["kind"] == "handoff" && r["variant"] == "blst+hashed")
        .map(|r| r["session_user_cycles"].as_u64().unwrap_or(0))
        .max()
        .unwrap_or(0);
    let key_figures = json!([
        {"what": "Miller loop over both pairs, blst (max over the phase cases)", "measured": miller},
        {"what": "final exponentiation and comparison with one, blst (max)", "measured": final_exp},
        {"what": "whole mainnet handoff update in one execution, blst + hashed committee (max of update[1..3], session user cycles)", "measured": handoff_whole},
        {"what": "everything before the Miller loop at 170 absent keys, blst + hashed committee (max)", "measured": begin_170},
    ]);

    // Step split (derived: sums of measured phases, blst + hashed; LEZ entry, message decode and job-state reads and
    // writes are not in the marks). Worst case over every real update and every phase case for the steps before
    // the final exponentiation; Finish over the real updates (a phase case is refused at Finish, so `apply` does not
    // run there).
    let blst_rows: Vec<&Value> = variant_rows
        .iter()
        .filter(|r| r["variant"] == "blst+hashed")
        .chain(phase_rows.iter().filter(|r| r["lib"] == "blst"))
        .collect();
    let sum_of = |r: &Value, ps: &[&str]| ps.iter().map(|k| get(r, k)).sum::<u64>();
    let worst = |ps: &[&str], real_only: bool| {
        blst_rows
            .iter()
            .filter(|r| !real_only || r.get("variant").is_some())
            .map(|r| sum_of(r, ps))
            .max()
            .unwrap_or(0)
    };
    let split = |name: &str, steps: &[(&str, &[&str], bool)]| {
        let rows: Vec<Value> = steps
            .iter()
            .map(|(step, ps, real_only)| {
                let w = worst(ps, *real_only);
                json!({"step": step, "phases": ps, "worst_marked_cycles": w, "share_of_cap": format!("{:.1}%", w as f64 * 100.0 / CAP as f64)})
            })
            .collect();
        json!({"layout": name, "steps": rows})
    };
    let step_split = json!([
        split(
            "Begin up to hash to G2",
            &[
                (
                    "Begin",
                    &["load", "decode", "checks", "aggregate", "sig_decode", "hash_to_g2"],
                    false
                ),
                ("Miller", &["miller"], false),
                ("Finish", &["final_exp", "apply"], true),
            ]
        ),
        split(
            "hash to G2 in Miller",
            &[
                ("Begin", &["load", "decode", "checks", "aggregate", "sig_decode"], false),
                ("Miller", &["hash_to_g2", "miller"], false),
                ("Finish", &["final_exp", "apply"], true),
            ]
        ),
        split(
            "signature decode and hash to G2 in Miller (the layout of lc_verify_v3)",
            &[
                ("Begin", &["load", "decode", "checks", "aggregate"], false),
                ("Miller", &["sig_decode", "hash_to_g2", "miller"], false),
                ("Finish", &["final_exp", "apply"], true),
            ]
        ),
    ]);

    let doc = json!({
        "tool": "tools/cost bench-bls (make bench-bls)",
        "measured": true,
        "executor": "RISC Zero 3.0.5 executor, no proof, no sequencer. Cycles are guest user cycles; phase cycles come from env::cycle_count marks inside the guest; session_user_cycles is the whole execution of one job (input decode included)",
        "guest": {"name": "bls_bench", "image_id": g.image_id, "elf_bytes": g.elf_bytes, "program_binary_bytes": g.program.len()},
        "fixtures": {"mainnet": "fixtures/mainnet: bootstrap_15605760, updates_1905_4, finality_update_15637644",
                     "sepolia": "fixtures/sepolia (Gloas): bootstrap_11534336, updates_1408_4, finality_update_11561824"},
        "phase_order": PHASES, "begin_phases": &PHASES[..BEGIN_PHASES],
        "cap": CAP,
        "v1_comparisons": v1_checks,
        "variants": variant_rows,
        "phase_cases": phase_rows,
        "key_figures": key_figures,
        "step_split_derived": step_split,
    });
    std::fs::create_dir_all(&out_dir)?;
    std::fs::write(
        format!("{out_dir}/phases.json"),
        serde_json::to_string_pretty(&doc)? + "\n",
    )?;
    std::fs::write(format!("{out_dir}/bench.md"), report(&doc))?;
    println!("wrote {out_dir}/phases.json and bench.md; {v1_checks} results equal V1");
    Ok(())
}

fn m(v: &Value) -> String {
    match v.as_u64() {
        Some(n) => format!("{:.2}M", n as f64 / 1e6),
        None => "n/a".into(),
    }
}

fn pct(v: &Value) -> String {
    v.as_u64()
        .map_or("n/a".into(), |n| format!("{:.1}%", n as f64 * 100.0 / CAP as f64))
}

fn report(doc: &Value) -> String {
    let mut s = String::new();
    let p = |s: &mut String, t: &str| {
        s.push_str(t);
        s.push('\n');
    };
    p(
        &mut s,
        "# BLS bench: zkcrypto against blst, hashed against cached committee (generated by `make bench-bls`; do not edit by hand)",
    );
    p(&mut s, "");
    p(
        &mut s,
        &format!(
            "Guest `bls_bench` image `{}` ({} user ELF bytes), RISC Zero 3.0.5 executor (no proof, no sequencer). All figures are **measured** guest user cycles on real fixtures; M = 10^6. Cap = LEZ rc3 public execution limit of {} per transaction.",
            doc["guest"]["image_id"].as_str().unwrap_or(""),
            doc["guest"]["elf_bytes"],
            doc["cap"]
        ),
    );
    p(&mut s, "");
    p(
        &mut s,
        "Libraries: **zkcrypto** = RISC Zero fork of `bls12_381` 0.8 (V2's BLS layer: committee keys decompressed from 48 bytes, with the subgroup check of `from_compressed`). **blst** = `crates/bls_blst` over RISC Zero fork of `blst` v0.3.16 (aggregate key and absent keys witnessed uncompressed, each checked on the curve and against its 48 committee bytes; signature subgroup-checked). Committee: **hashed** = the signing committee's root recomputed from its 24,624 bytes (direct SHA-256); **cached** = root taken as given (lower bound for a committee kept on chain). The other committee enters as its 32-byte root in all four. `v2_today` = `attest_core::lc::verify_and_apply_v2` unchanged (decodes and hashes both committees).",
    );
    p(&mut s, "");
    p(
        &mut s,
        &format!(
            "Every result below equals V1's (Helios 0.12.0 unmodified, native): {} comparisons, all equal (accept with the same pre-store, post-store, head and record; or the same refusal code).",
            doc["v1_comparisons"]
        ),
    );
    p(&mut s, "");
    p(&mut s, "## 1. Key figures");
    p(&mut s, "");
    p(&mut s, "| what | measured |");
    p(&mut s, "|---|---|");
    for l in doc["key_figures"].as_array().into_iter().flatten() {
        p(
            &mut s,
            &format!("| {} | {} |", l["what"].as_str().unwrap_or(""), m(&l["measured"])),
        );
    }
    p(&mut s, "");
    p(&mut s, "## 1b. Three-step split (derived)");
    p(&mut s, "");
    p(
        &mut s,
        "Sums of the measured phases of section 3 and 4, blst + hashed committee, worst case over all real updates and all phase cases (0, 1, 170 absent keys); Finish over the real updates only. **Derived**: the LEZ entry, message decode and job-state reads and writes of the staged program are not included; `exec-steps.json` next to this folder has the steps of `lc_verify_v3` itself, metered.",
    );
    p(&mut s, "");
    p(&mut s, "| layout | step | phases | worst case | share of cap |");
    p(&mut s, "|---|---|---|---|---|");
    for l in doc["step_split_derived"].as_array().into_iter().flatten() {
        for st in l["steps"].as_array().into_iter().flatten() {
            let ps: Vec<&str> = st["phases"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            p(
                &mut s,
                &format!(
                    "| {} | {} | {} | {} | {} |",
                    l["layout"].as_str().unwrap_or(""),
                    st["step"].as_str().unwrap_or(""),
                    ps.join(", "),
                    m(&st["worst_marked_cycles"]),
                    st["share_of_cap"].as_str().unwrap_or("")
                ),
            );
        }
    }
    p(&mut s, "");
    p(
        &mut s,
        "## 2. Whole update in one execution: four variants and today's V2",
    );
    p(&mut s, "");
    p(
        &mut s,
        "Session user cycles (whole execution, input decode included). Share of the cap in brackets for the blst rows.",
    );
    p(&mut s, "");
    p(
        &mut s,
        "| network | update | kind | signers | zkcrypto + hashed | zkcrypto + cached | blst + hashed | blst + cached | V2 today |",
    );
    p(&mut s, "|---|---|---|---|---|---|---|---|---|");
    let vrows = doc["variants"].as_array().cloned().unwrap_or_default();
    let mut seen: Vec<(String, String)> = Vec::new();
    for r in &vrows {
        let key = (
            r["network"].as_str().unwrap_or("").to_string(),
            r["update"].as_str().unwrap_or("").to_string(),
        );
        if seen.contains(&key) {
            continue;
        }
        seen.push(key.clone());
        let cell = |v: &str| {
            vrows
                .iter()
                .find(|x| x["network"] == key.0.as_str() && x["update"] == key.1.as_str() && x["variant"] == v)
                .map(|x| x["session_user_cycles"].clone())
                .unwrap_or(Value::Null)
        };
        p(
            &mut s,
            &format!(
                "| {} | {} | {} | {} | {} | {} | {} ({}) | {} ({}) | {} |",
                key.0,
                key.1,
                r["kind"].as_str().unwrap_or(""),
                r["participation"],
                m(&cell("zkcrypto+hashed")),
                m(&cell("zkcrypto+cached")),
                m(&cell("blst+hashed")),
                pct(&cell("blst+hashed")),
                m(&cell("blst+cached")),
                pct(&cell("blst+cached")),
                m(&cell("v2_today")),
            ),
        );
    }
    p(&mut s, "");
    p(
        &mut s,
        "## 3. Phases of the whole update, blst + hashed committee, real participation",
    );
    p(&mut s, "");
    p(
        &mut s,
        "Marked cycles per phase (`load` = committee root; `decode` = update decode, quorum, pre-store commitment; `checks` = Helios rule checks, branches, next-committee root, signing root; `apply` = Helios apply on roots, post-store, head). `begin` = everything before the Miller loop.",
    );
    p(&mut s, "");
    let mut hdr = String::from("| network | update | kind |");
    let mut sep = String::from("|---|---|---|");
    for ph in PHASES {
        hdr.push_str(&format!(" {ph} |"));
        sep.push_str("---|");
    }
    hdr.push_str(" begin | total marked |");
    sep.push_str("---|---|");
    p(&mut s, &hdr);
    p(&mut s, &sep);
    for r in vrows.iter().filter(|r| r["variant"] == "blst+hashed") {
        let mut line = format!(
            "| {} | {} | {} |",
            r["network"].as_str().unwrap_or(""),
            r["update"].as_str().unwrap_or(""),
            r["kind"].as_str().unwrap_or("")
        );
        for ph in PHASES {
            line.push_str(&format!(" {} |", m(&r["phases"][ph])));
        }
        line.push_str(&format!(" {} | {} |", m(&r["begin_cycles"]), m(&r["marked_cycles"])));
        p(&mut s, &line);
    }
    p(&mut s, "");
    p(
        &mut s,
        "## 4. Phases with 0, 1 and 170 absent keys, zkcrypto against blst (hashed committee)",
    );
    p(&mut s, "");
    p(
        &mut s,
        "Participation bits rewritten (all set, then the first k cleared); the signature then fails at the final exponentiation (refusal E03 on V1 and here), so `apply` does not run. The phase costs before that are what is measured.",
    );
    p(&mut s, "");
    p(
        &mut s,
        "| network | update | absent | lib | load | decode | checks | aggregate | sig_decode | hash_to_g2 | miller | final_exp | begin | verdict |",
    );
    p(&mut s, "|---|---|---|---|---|---|---|---|---|---|---|---|---|---|");
    for r in doc["phase_cases"].as_array().into_iter().flatten() {
        let mut line = format!(
            "| {} | {} | {} | {} |",
            r["network"].as_str().unwrap_or(""),
            r["update"].as_str().unwrap_or(""),
            r["absent_keys"],
            r["lib"].as_str().unwrap_or("")
        );
        for ph in &PHASES[..8] {
            line.push_str(&format!(" {} |", m(&r["phases"][*ph])));
        }
        line.push_str(&format!(
            " {} | {} |",
            m(&r["begin_cycles"]),
            r["verdict"].as_str().unwrap_or("")
        ));
        p(&mut s, &line);
    }
    p(&mut s, "");
    p(
        &mut s,
        "Steps of the deployed `lc_verify_v3` as public transactions, executor and on chain side by side: \
         `evidence/cost/cost.md` section 8 (per transaction the charge equals the executor's cycles). In \
         `lc_verify_v3` the program's entry, message and job-state handling take about 0.02M to 0.3M per step \
         beyond its marked phases (`unmarked_cycles` in `evidence/public-path/exec-steps.json`); it also builds the \
         bigint shim at opt-level 3, which this bench guest does not (`docs/DECISIONS.md` D-56), so its BLS phases \
         are lower than the ones above.",
    );
    p(&mut s, "");
    p(&mut s, "## 5. How to re-run");
    p(&mut s, "");
    p(
        &mut s,
        "`BENCH_ROUTE=docker make bench-bls` (guest built in `risczero/risc0-guest-builder:r0.1.91.1` from the committed tree; the image id above is that build's) or `make bench-bls` (local rzup toolchain; image id differs, cycles may differ slightly). Raw data: `phases.json` next to this file; build record: `build.log`.",
    );
    s
}
