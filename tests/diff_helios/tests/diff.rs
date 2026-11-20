//! The differential test: runs every case, writes `evidence/diff/*`, and asserts the pass
//! rule and the coverage floors.

use diff_helios::{default_columns, run_and_report, threads};

#[test]
fn helios_v1_v2_differential() {
    let s = run_and_report(&default_columns(), threads()).expect("run");
    let j = &s.json;
    assert!(s.failures.is_empty(), "failures:\n{}", s.failures.join("\n"));
    assert!(j["total"].as_u64().unwrap() >= 300, "total {}", j["total"]);
    assert!(j["valid"].as_u64().unwrap() >= 30, "valid {}", j["valid"]);
    assert_eq!(j["distinct_inputs"], j["total"], "every case is a distinct input");
    for n in ["mainnet", "sepolia", "hoodi"] {
        assert!(j["networks"][n]["valid"].as_u64().unwrap_or(0) > 0, "no valid {n} case");
    }
    assert!(j["gloas_cases"].as_u64().unwrap() >= 60, "gloas {}", j["gloas_cases"]);
    assert!(j["fork_crossing_valid"].as_u64().unwrap() >= 1, "no valid crossing");
    assert_eq!(j["v1_v2_disagreements"], 0);
    // The executor pass (`diff_exec`, evidence/diff/executor.json): every case through the lc_verify_v3 guest, each
    // equal to V1 except the designed difference, and no case newer than the recorded run.
    let e = &j["executor"];
    assert!(e.is_object(), "evidence/diff/executor.json missing: run diff_exec");
    assert_eq!(
        e["missing"], 0,
        "cases without an executor result for their current input"
    );
    assert!(j["executor_cases"].as_u64().unwrap() >= j["valid"].as_u64().unwrap() + 100);
    assert_eq!(j["executor_mismatches"], 0);
    assert_eq!(
        e["native_v3_compared"], true,
        "the v3 column must run beside the executor pass"
    );
    assert_eq!(
        e["native_v3_mismatches"].as_array().map(Vec::len),
        Some(0),
        "guest and native V3 differ"
    );
    assert_eq!(
        e["image_id_matches_expected"], true,
        "executor image is not the pinned lc_verify_v3 image"
    );
    // The executor's designed differences are exactly the native V3 column's.
    let ids = |v: &serde_json::Value, column: Option<&str>| -> Vec<String> {
        v.as_array()
            .unwrap()
            .iter()
            .filter(|d| column.is_none_or(|c| d["column"] == c))
            .map(|d| d["id"].as_str().unwrap().to_string())
            .collect()
    };
    let designed = ids(&e["designed_differences"], None);
    assert!(designed.contains(&"fabricated/identity_member_counted".to_string()));
    assert_eq!(designed, ids(&j["intended_divergences"], Some("v3")));
    // The deliberate refusals: sub-quorum (E04) and no finality (E02) are each present.
    for r in ["E02", "E04"] {
        assert!(
            j["helios_accepts_we_refuse"]["by_reason"][r].as_u64().unwrap_or(0) > 0,
            "{r}"
        );
    }
}

/// The harness reports a column that differs from V1 only in the post-store commitment of accepted updates.
#[test]
fn harness_catches_a_wrong_post_store() {
    use attest_core::{InstanceConfig, lc::StoreWitness};
    use diff_helios::{Column, V1, Verdict, all_cases, report, run};

    struct WrongPostStore;
    impl Column for WrongPostStore {
        fn name(&self) -> &'static str {
            "wrong"
        }
        fn verify(&self, config: &InstanceConfig, witness: &StoreWitness, update_ssz: &[u8]) -> Verdict {
            match V1.verify(config, witness, update_ssz) {
                Verdict::Accept(mut a) => {
                    a.post_store[0] ^= 1;
                    Verdict::Accept(a)
                }
                other => other,
            }
        }
    }

    let cases: Vec<_> = all_cases().expect("cases").into_iter().take(40).collect();
    let out = run(&cases, &[&V1, &WrongPostStore], 2);
    let s = report::summarize(&cases, &out, &["v1", "wrong"], &serde_json::json!({}));
    let accepted = out
        .iter()
        .filter(|o| matches!(o.verdicts[0], Verdict::Accept(_)))
        .count();
    assert!(accepted > 0);
    assert_eq!(s.json["disagreements_with_v1"]["wrong"], accepted);
    assert_eq!(s.failures.len(), accepted);
    assert_eq!(s.json["pass"], false);
}

/// Unmodified Helios `verify_*` passes each of these fabricated updates (valid signatures, our own keys); our
/// pre-checks refuse all but the honest and 342-signer ones before Helios runs: sub-quorum (E04) and no finality (E02,
/// also with a non-zero finality branch that Helios never checks, a16z/helios #872).
#[test]
fn prechecks_close_the_helios_gap() {
    use attest_core::EsaError::{E02NotFinalised, E04LowParticipation};
    use diff_helios::{Column, Expect, V1, Verdict, all_cases, helios_raw};

    let cases = all_cases().expect("cases");
    let want = [
        ("chain0", Expect::Accept),
        ("participation342", Expect::Accept),
        ("participation341", Expect::Refuse(E04LowParticipation)),
        ("participation1", Expect::Refuse(E04LowParticipation)),
        ("finality_one_signer", Expect::Refuse(E04LowParticipation)),
        ("no_finality", Expect::Refuse(E02NotFinalised)),
        ("no_finality_one_signer", Expect::Refuse(E02NotFinalised)),
        ("empty_finalized_junk_branch", Expect::Refuse(E02NotFinalised)),
    ];
    for (id, expect) in want {
        let c = cases
            .iter()
            .find(|c| c.id == format!("fabricated/{id}"))
            .unwrap_or_else(|| panic!("no case {id}"));
        let h = helios_raw(c);
        assert!(h.accepted(), "{id}: Helios verify {}", h.verify);
        let v = V1.verify(&c.config, &c.witness, &c.update);
        assert!(v.matches(expect), "{id}: V1 {}", v.short());
        assert_eq!(
            matches!(v, Verdict::Accept(_)),
            h.store_moved,
            "{id}: Helios apply moved the store"
        );
    }
}
