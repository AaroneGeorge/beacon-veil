//! `evidence/diff/{report.json,report.md,cases.tsv,run.log}`: what the run saw, deterministic apart from
//! `generated_at_utc` (and the timings in `run.log`).

use std::{collections::BTreeMap, fmt::Write as _, path::Path};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{Case, Kind, Outcome, Verdict, cases::TAMPER_CLASSES};

/// Analysis of one run.
pub struct Summary {
    pub json: Value,
    pub markdown: String,
    pub tsv: String,
    /// Everything that breaks the pass rule (empty on a pass).
    pub failures: Vec<String>,
}

/// UTC time as `YYYY-MM-DDTHH:MM:SSZ` (civil-from-days, no extra dependency).
#[must_use]
pub fn utc_now() -> String {
    let s = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let (days, rem) = (s / 86_400, s % 86_400);
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

type Counts = BTreeMap<String, u64>;

fn inc(m: &mut Counts, k: impl Into<String>) {
    *m.entry(k.into()).or_default() += 1;
}

/// Whether `divergence` applies to a column: it describes a designed difference of the staged verifier (V3).
fn divergence_applies(column: &str) -> bool {
    column.starts_with("v3")
}

#[must_use]
pub fn summarize(cases: &[Case], outcomes: &[Outcome], columns: &[&str], fixtures: &Value) -> Summary {
    assert_eq!(cases.len(), outcomes.len());
    let mut failures = Vec::new();
    let mut by_kind = BTreeMap::new();
    let mut networks: BTreeMap<String, BTreeMap<String, u64>> = BTreeMap::new();
    // class -> (cases, expected codes, V1 verdicts, Helios verify results)
    let mut by_class: BTreeMap<&str, (u64, Counts, Counts, Counts)> = BTreeMap::new();
    let mut disagreements: BTreeMap<&str, Vec<Value>> = columns[1..].iter().map(|c| (*c, Vec::new())).collect();
    let mut intended = Vec::new();
    let mut deviations = Vec::new();
    let mut panics = Vec::new();
    let mut accepts_we_refuse: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut refuses_we_accept = Vec::new();
    let mut inputs = Sha256::new();
    let mut distinct = std::collections::BTreeSet::new();
    let mut network_forks: BTreeMap<String, std::collections::BTreeSet<&str>> = BTreeMap::new();
    let (mut gloas, mut crossing_cases, mut crossing_valid) = (0u64, 0u64, 0u64);

    let mut tsv = String::from(
        "id\tnetwork\tfork\tkind\tclass\tcrossing\tsource\tinput_sha256\texpected\thelios_verify\thelios_store_moved",
    );
    for c in columns {
        let _ = write!(tsv, "\t{c}");
    }
    tsv.push_str("\tall_agree_with_v1\n");

    for (c, o) in cases.iter().zip(outcomes) {
        let sha = c.input_sha256();
        inputs.update(sha.as_bytes());
        distinct.insert(sha.clone());
        network_forks.entry(c.network.clone()).or_default().insert(c.fork);
        inc(&mut by_kind, c.kind.name());
        let n = networks.entry(c.network.clone()).or_default();
        inc(n, "total");
        inc(n, c.kind.name());
        if c.fork == "gloas" {
            gloas += 1;
            inc(n, "gloas");
        }
        if c.crossing {
            crossing_cases += 1;
            crossing_valid += u64::from(c.kind == Kind::Valid);
        }
        let v1 = &o.verdicts[0];
        let e = by_class.entry(c.class).or_default();
        e.0 += 1;
        inc(&mut e.1, c.expect.short());
        inc(
            &mut e.2,
            if matches!(v1, Verdict::Accept(_)) {
                "accept".into()
            } else {
                v1.short()
            },
        );
        inc(&mut e.3, o.helios.verify.clone());
        if o.helios.store_moved {
            inc(&mut e.3, "ok, store moved");
        }

        if !v1.matches(c.expect) {
            deviations.push(json!({"id": c.id, "expected": c.expect.short(), "v1": v1.short()}));
            failures.push(format!("{}: expected {}, V1 {}", c.id, c.expect.short(), v1.short()));
        }
        let mut all_agree = true;
        for (i, v) in o.verdicts.iter().enumerate() {
            if let Verdict::Panic(m) = v {
                panics.push(json!({"id": c.id, "column": columns[i], "message": m}));
                failures.push(format!("{}: {} panicked: {m}", c.id, columns[i]));
            }
            if i == 0 || v == v1 {
                continue;
            }
            all_agree = false;
            let row = json!({"id": c.id, "column": columns[i], "v1": v1.short(), "other": v.short()});
            match c.divergence {
                Some(reason) if divergence_applies(columns[i]) => {
                    let mut r = row;
                    r["reason"] = json!(reason);
                    intended.push(r);
                }
                _ => {
                    failures.push(format!("{}: {} {} but v1 {}", c.id, columns[i], v.short(), v1.short()));
                    disagreements.get_mut(columns[i]).expect("column").push(row);
                }
            }
        }
        match (o.helios.accepted(), v1) {
            (true, Verdict::Refuse(e)) => accepts_we_refuse
                .entry(e.code().to_string())
                .or_default()
                .push(c.id.clone()),
            (false, Verdict::Accept(_)) => refuses_we_accept.push(json!({"id": c.id, "helios": o.helios.verify})),
            _ => {}
        }

        let _ = write!(
            tsv,
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            c.id,
            c.network,
            c.fork,
            c.kind.name(),
            c.class,
            c.crossing,
            c.source,
            sha,
            c.expect.short(),
            o.helios.verify,
            o.helios.store_moved
        );
        for v in &o.verdicts {
            let _ = write!(tsv, "\t{}", v.short());
        }
        let _ = writeln!(tsv, "\t{all_agree}");
    }
    if !refuses_we_accept.is_empty() {
        failures.push(format!(
            "{} cases Helios refuses and V1 accepts",
            refuses_we_accept.len()
        ));
    }

    let col_dis: BTreeMap<String, Value> = disagreements
        .iter()
        .map(|(k, v)| ((*k).to_string(), json!(v.len())))
        .collect();
    let v1_v2 = disagreements.get("v2").map(Vec::len);
    let v1_v3 = disagreements.get("v3").map(Vec::len);
    let classes: BTreeMap<&str, Value> = by_class
        .iter()
        .map(|(k, (n, expected, v1, h))| {
            (
                *k,
                json!({"cases": n, "expected": expected, "v1_seen": v1, "helios_verify": h}),
            )
        })
        .collect();
    let valid = by_kind.get("valid").copied().unwrap_or(0);
    let fab_valid = cases
        .iter()
        .filter(|c| c.kind == Kind::Fabricated && c.expect == crate::Expect::Accept)
        .count();
    let json = json!({
        "what": "Differential test: unmodified Helios 0.12.0 verify_*/apply_* and each verifier column on real, tampered and fabricated light-client updates. Reference column: v1.",
        "generated_at_utc": utc_now(),
        "helios": "a16z/helios tag 0.12.0 (2af9b41), helios-consensus-core unmodified",
        "columns": columns,
        "total": cases.len(),
        "valid": valid,
        "valid_fabricated_positive_controls": fab_valid,
        "by_kind": by_kind,
        "distinct_inputs": distinct.len(),
        "networks": networks,
        "network_forks": network_forks,
        "gloas_cases": gloas,
        "fork_crossing": "sepolia",
        "fork_crossing_detail": "Sepolia period 1378 (Fulu) to 1379 (Gloas, epoch 353024); Hoodi (Fulu) has no Gloas fork scheduled (fork_schedule GLOAS epoch = far future at fetch time)",
        "fork_crossing_cases": crossing_cases,
        "fork_crossing_valid": crossing_valid,
        "by_class": classes,
        "disagreements_with_v1": col_dis,
        "v1_v2_disagreements": v1_v2,
        "v1_v3_disagreements": v1_v3,
        "disagreement_cases": disagreements,
        "intended_divergences": intended,
        "expected_mismatches": deviations,
        "panics": panics,
        "helios_accepts_we_refuse": {
            "count": accepts_we_refuse.values().map(Vec::len).sum::<usize>(),
            "by_reason": accepts_we_refuse.iter().map(|(k, v)| (k.clone(), json!(v.len()))).collect::<BTreeMap<_, _>>(),
            "cases": accepts_we_refuse,
        },
        "helios_refuses_we_accept": refuses_we_accept,
        "executor_cases": Value::Null,
        "executor_mismatches": Value::Null,
        "inputs_sha256": hex::encode(inputs.finalize()),
        "fixtures": fixtures,
        "pass": failures.is_empty(),
    });
    let markdown = markdown(&json, columns);
    Summary {
        json,
        markdown,
        tsv,
        failures,
    }
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

fn markdown(j: &Value, columns: &[&str]) -> String {
    let mut m = String::new();
    let _ = writeln!(m, "# Differential test: Helios 0.12.0 against our verifiers\n");
    let _ = writeln!(
        m,
        "Generated {} by `cargo nextest run -p diff_helios` (or `cargo run -p diff_helios --bin diff_report`). \
         Every row of `cases.tsv` is one case with the SHA-256 of its input (config, store witness, update bytes).\n",
        j["generated_at_utc"].as_str().unwrap_or_default()
    );
    let _ = writeln!(
        m,
        "Pass rule: every column agrees with V1 on every case (accept or refuse, error code, and on accept the \
         pre-store and post-store commitments, head and record). Columns: {}.\n",
        columns.join(", ")
    );
    let _ = writeln!(m, "## Totals\n");
    let _ = writeln!(m, "| | |\n|---|---|");
    for k in [
        "total",
        "distinct_inputs",
        "valid",
        "valid_fabricated_positive_controls",
        "gloas_cases",
        "fork_crossing",
        "fork_crossing_valid",
        "fork_crossing_cases",
        "v1_v2_disagreements",
        "v1_v3_disagreements",
        "executor_cases",
        "executor_mismatches",
        "pass",
    ] {
        let _ = writeln!(m, "| {k} | {} |", j[k]);
    }
    let _ = writeln!(m, "\n## Cases by network\n");
    let _ = writeln!(
        m,
        "| network | total | valid | tampered | optimistic | fabricated | gloas |\n|---|---|---|---|---|---|---|"
    );
    if let Some(nets) = j["networks"].as_object() {
        for (n, v) in nets {
            let forks: Vec<String> = j["network_forks"][n.as_str()]
                .as_array()
                .map(|a| a.iter().filter_map(|f| f.as_str()).map(capitalise).collect())
                .unwrap_or_default();
            let n = format!("{n} ({})", forks.join(", "));
            let g = |k: &str| v[k].as_u64().unwrap_or(0);
            let _ = writeln!(
                m,
                "| {n} | {} | {} | {} | {} | {} | {} |",
                g("total"),
                g("valid"),
                g("tampered"),
                g("optimistic"),
                g("fabricated"),
                g("gloas")
            );
        }
    }
    let _ = writeln!(m, "\n## Cases by class\n");
    let _ = writeln!(
        m,
        "\"Expected\" is the code the test asserts; \"V1 seen\" is what V1 returned (they must match); \"Helios verify\" is unmodified Helios `verify_*` on the same input.\n"
    );
    let _ = writeln!(
        m,
        "| class | cases | expected | V1 seen | Helios verify |\n|---|---|---|---|---|"
    );
    let order: Vec<&str> = [
        "valid_handoff_update",
        "valid_handoff_finality",
        "valid_learn_next",
        "valid_same_period_finality",
    ]
    .into_iter()
    .chain(TAMPER_CLASSES.iter().map(|(c, _)| *c))
    .collect();
    if let Some(cl) = j["by_class"].as_object() {
        let mut keys: Vec<&String> = cl.keys().collect();
        keys.sort_by_key(|k| (order.iter().position(|o| o == k).unwrap_or(usize::MAX), (*k).clone()));
        for k in keys {
            let v = &cl[k];
            let fmt = |x: &Value| {
                x.as_object()
                    .map(|o| {
                        o.iter()
                            .map(|(a, b)| format!("{a} x{b}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default()
            };
            let _ = writeln!(
                m,
                "| {k} | {} | {} | {} | {} |",
                v["cases"],
                fmt(&v["expected"]),
                fmt(&v["v1_seen"]),
                fmt(&v["helios_verify"])
            );
        }
    }
    let _ = writeln!(m, "\n## Where unmodified Helios `verify_*` accepts and we refuse\n");
    let _ = writeln!(
        m,
        "These are deliberate. E04: fewer than 342 signers (Helios `verify_*` needs one signer; its `apply_*` then skips the update without an error). E02: no finalized header (an optimistic update, or a full or finality update whose finalized header is all zero: Helios reads a zero header as \"no finality\" (`default_header_to_none`, as the consensus spec does) and `verify_*` passes it as a non-finality update). E14: the update moves nothing (a replay on the store it already produced: Helios `verify_*` passes and `apply_*` changes nothing).\n"
    );
    let _ = writeln!(m, "| reason | cases |\n|---|---|");
    if let Some(r) = j["helios_accepts_we_refuse"]["by_reason"].as_object() {
        for (k, v) in r {
            let _ = writeln!(m, "| {k} | {v} |");
        }
    }
    let list = |m: &mut String, title: &str, v: &Value| {
        let _ = writeln!(m, "\n## {title}\n");
        match v.as_array() {
            Some(a) if !a.is_empty() => {
                for x in a {
                    let _ = writeln!(m, "- `{x}`");
                }
            }
            _ => {
                let _ = writeln!(m, "None.");
            }
        }
    };
    let flat: Vec<Value> = j["disagreement_cases"]
        .as_object()
        .map(|o| {
            o.values()
                .flat_map(|v| v.as_array().cloned().unwrap_or_default())
                .collect()
        })
        .unwrap_or_default();
    list(&mut m, "Disagreements with V1", &Value::Array(flat));
    list(
        &mut m,
        "Designed differences (listed, not counted as agreement)",
        &j["intended_divergences"],
    );
    list(&mut m, "V1 different from the expected code", &j["expected_mismatches"]);
    list(&mut m, "Helios refuses, V1 accepts", &j["helios_refuses_we_accept"]);
    list(&mut m, "Panics", &j["panics"]);
    if j["executor"].is_object() {
        let e = &j["executor"];
        let _ = writeln!(m, "\n## Executor pass\n");
        let _ = writeln!(
            m,
            "Each case also ran through the `lc_verify_v3` guest as one `VerifyOnce` message in the RISC Zero \
             executor ({}; no proof), framed as LEZ frames an actor input. User ELF sha256 `{}`, image id `{}`. \
             A case counts when its input hash equals the one recorded by the executor run.\n",
            e["executor"].as_str().unwrap_or_default(),
            e["elf_sha256"].as_str().unwrap_or_default(),
            e["image_id"].as_str().unwrap_or_default()
        );
        let _ = writeln!(m, "| | |\n|---|---|");
        for k in [
            "cases",
            "valid_cases",
            "accepted",
            "refused",
            "mismatches",
            "missing",
            "max_cycles_accepted",
            "image_id_matches_expected",
        ] {
            let _ = writeln!(m, "| {k} | {} |", e[k]);
        }
        list(&mut m, "Executor results different from V1", &e["mismatch_cases"]);
        list(
            &mut m,
            "Executor results different from the native V3 column",
            &e["native_v3_mismatches"],
        );
        list(&mut m, "Executor designed differences", &e["designed_differences"]);
    }
    m
}

/// SHA-256 of each network's `manifest.json` under `fixtures/diff`.
#[must_use]
pub fn fixture_manifests(dir: &Path) -> Value {
    let mut m = BTreeMap::new();
    for n in ["mainnet", "sepolia", "hoodi"] {
        if let Ok(b) = std::fs::read(dir.join(n).join("manifest.json")) {
            m.insert(n.to_string(), json!(hex::encode(Sha256::digest(&b))));
        }
    }
    json!(m)
}

/// Writes `report.json`, `report.md`, `cases.tsv` and `run.log` into `dir`.
pub fn write(dir: &Path, s: &Summary, log: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join("report.json"), serde_json::to_string_pretty(&s.json)? + "\n")?;
    std::fs::write(dir.join("report.md"), &s.markdown)?;
    std::fs::write(dir.join("cases.tsv"), &s.tsv)?;
    std::fs::write(dir.join("run.log"), log)?;
    Ok(())
}

/// `evidence/diff` of this workspace (respects `BV_REPO`).
#[must_use]
pub fn evidence_dir() -> std::path::PathBuf {
    fixtures::fixtures_dir().join("../evidence/diff")
}

/// Merges an executor run (`evidence/diff/executor.json`, written by `diff_exec`) into the summary: every case whose
/// input hash equals the recorded one is compared with V1 by [`Verdict::digest`]. A difference is a failure unless
/// the case carries a designed difference for `v3*` columns (the executor column is `v3_exec`).
pub fn add_executor(s: &mut Summary, cases: &[Case], outcomes: &[Outcome], columns: &[&str], exec: &Value) {
    let by_id: BTreeMap<&str, &Value> = exec["cases"]
        .as_array()
        .map(|a| a.iter().filter_map(|e| Some((e["id"].as_str()?, e))).collect())
        .unwrap_or_default();
    let (mut n, mut valid, mut accepted, mut refused, mut missing, mut max_cycles) =
        (0u64, 0u64, 0u64, 0u64, 0u64, 0u64);
    let mut mismatches = Vec::new();
    let mut designed = Vec::new();
    // The guest runs the code of the native V3 column: on every case their verdicts must be equal, designed
    // differences from V1 included (a guest that accepted a designed-difference case would differ here).
    let v3_col = columns.iter().position(|c| *c == "v3");
    let mut native_mismatches = Vec::new();
    for (c, o) in cases.iter().zip(outcomes) {
        let Some(e) = by_id
            .get(c.id.as_str())
            .filter(|e| e["input_sha256"].as_str() == Some(&c.input_sha256()))
        else {
            missing += 1;
            continue;
        };
        n += 1;
        valid += u64::from(c.kind == Kind::Valid);
        let got = e["digest"].as_str().unwrap_or_default();
        let cycles = e["cycles"].as_u64().unwrap_or(0);
        if got.starts_with("accept:") {
            accepted += 1;
            max_cycles = max_cycles.max(cycles);
        } else {
            refused += 1;
        }
        if let Some(i) = v3_col {
            let native = o.verdicts[i].digest();
            if got != native {
                s.failures.push(format!(
                    "{}: executor {} but native v3 {}",
                    c.id,
                    e["verdict"],
                    o.verdicts[i].short()
                ));
                native_mismatches.push(json!({"id": c.id, "v3": o.verdicts[i].short(), "executor": e["verdict"]}));
            }
        }
        let want = o.verdicts[0].digest();
        if got != want {
            let row = json!({"id": c.id, "v1": o.verdicts[0].short(), "executor": e["verdict"]});
            match c.divergence {
                Some(reason) if divergence_applies("v3_exec") => {
                    let mut r = row;
                    r["reason"] = json!(reason);
                    designed.push(r);
                }
                _ => {
                    s.failures.push(format!(
                        "{}: executor {} but v1 {}",
                        c.id,
                        e["verdict"],
                        o.verdicts[0].short()
                    ));
                    mismatches.push(row);
                }
            }
        }
    }
    // The executor ran the released guest: its image id is the one guests/IMAGE_IDS.expected pins.
    let expected_image = expected_image_id("lc_verify_v3");
    let image_matches = expected_image.is_some() && exec["image_id"].as_str() == expected_image.as_deref();
    if !image_matches {
        s.failures.push(format!(
            "executor image {} is not the lc_verify_v3 image of guests/IMAGE_IDS.expected ({})",
            exec["image_id"],
            expected_image.as_deref().unwrap_or("missing")
        ));
    }
    let j = &mut s.json;
    j["executor_cases"] = json!(n);
    j["executor_mismatches"] = json!(mismatches.len());
    j["executor"] = json!({
        "executor": exec["executor"],
        "elf": exec["elf"],
        "elf_sha256": exec["elf_sha256"],
        "image_id": exec["image_id"],
        "generated_at_utc": exec["generated_at_utc"],
        "cases": n,
        "valid_cases": valid,
        "accepted": accepted,
        "refused": refused,
        "missing": missing,
        "max_cycles_accepted": max_cycles,
        "mismatches": mismatches.len(),
        "mismatch_cases": mismatches,
        "designed_differences": designed,
        "native_v3_compared": v3_col.is_some(),
        "native_v3_mismatches": native_mismatches,
        "expected_image_id": expected_image,
        "image_id_matches_expected": image_matches,
    });
    j["pass"] = json!(s.failures.is_empty());
    s.markdown = markdown(&s.json, columns);
}

/// The image id pinned for `program` in `guests/IMAGE_IDS.expected` (`<program> <hex>` lines).
#[must_use]
pub fn expected_image_id(program: &str) -> Option<String> {
    let text = std::fs::read_to_string(fixtures::fixtures_dir().join("../guests/IMAGE_IDS.expected")).ok()?;
    text.lines().filter(|l| !l.starts_with('#')).find_map(|l| {
        let mut it = l.split_whitespace();
        (it.next() == Some(program))
            .then(|| it.next().map(str::to_string))
            .flatten()
    })
}
