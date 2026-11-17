//! Cycles of `attest::Attest` (log and storage claims) and of `consumer_erc20` in the RISC Zero executor (for
//! the cost table). No sequencer: each handler is run once with a `ReceiveInput` frame, as LEZ runs an actor.
//!
//! ```text
//! cargo run -p e2e_consumer --bin attest_exec -- [--attest-elf P] [--consumer-elf P] [--json OUT]
//! cargo run -p e2e_consumer --bin attest_exec -- --to-bin ELF OUT.bin
//! ```
//! Default ELFs: `$CARGO_TARGET_DIR/riscv-guest/guest_methods/<pkg>/riscv32im-risc0-zkvm-elf/release/<bin>`.
//! Cycles are deterministic for a given ELF and input; `user_cycles` is what LEZ meters as gas (1 gas per cycle).

use std::time::Instant;

use anyhow::{Context, Result, anyhow};
use attest_core::{AccountId, AttestMessage, Predicate, PredicateValue};
use consumer_erc20_program::{ConsumerState, consumer_instance_id};
use e2e_consumer::{Fixture, MAINNET_RECORD_SLOT};
use lee_core::{
    account::{Actor, ActorState},
    program::ReceiveInput,
};
use risc0_zkvm::{ExecutorEnv, default_executor};
use serde_json::json;

fn guest_path(pkg: &str, bin: &str) -> String {
    let target = std::env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| "target".into());
    format!("{target}/riscv-guest/guest_methods/{pkg}/riscv32im-risc0-zkvm-elf/release/{bin}")
}

fn load_program(path: &str) -> Result<(Vec<u8>, Option<usize>)> {
    let raw = std::fs::read(path).with_context(|| format!("reading {path}"))?;
    if path.ends_with(".bin") {
        Ok((raw, None))
    } else {
        let n = raw.len();
        Ok((
            risc0_binfmt::ProgramBinary::new(&raw, risc0_zkos_v1compat::V1COMPAT_ELF).encode(),
            Some(n),
        ))
    }
}

struct Run {
    user_cycles: u64,
    total_cycles: u64,
    segments: usize,
    ms: u128,
    ok: bool,
}

fn execute(program: &[u8], input: &ReceiveInput) -> Result<Run> {
    let frame = lee_core::to_frame(&borsh::to_vec(input)?);
    let env = ExecutorEnv::builder().write_slice(&frame).build()?;
    let t = Instant::now();
    let r = default_executor().execute(env, program);
    let ms = t.elapsed().as_millis();
    match r {
        Ok(info) => Ok(Run {
            user_cycles: info.cycles(),
            total_cycles: info.segments.iter().map(|s| 1u64 << s.po2).sum(),
            segments: info.segments.len(),
            ms,
            ok: true,
        }),
        Err(e) => {
            println!("  guest refused: {e:#}");
            Ok(Run {
                user_cycles: 0,
                total_cycles: 0,
                segments: 0,
                ms,
                ok: false,
            })
        }
    }
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let arg = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    // `--to-bin ELF OUT`: write the LEZ-deployable program binary (ProgramBinary with the v1compat kernel), as
    // risc0-build's `.bin`, for deploying a guest built outside crates/guest_methods.
    if let Some(i) = args.iter().position(|a| a == "--to-bin") {
        let (elf, out) = (
            args.get(i + 1).context("--to-bin ELF OUT")?,
            args.get(i + 2).context("--to-bin ELF OUT")?,
        );
        let (bin, _) = load_program(elf)?;
        std::fs::write(out, &bin)?;
        println!(
            "{out}: {} bytes, image id {}",
            bin.len(),
            risc0_binfmt::compute_image_id(&bin).map_err(|e| anyhow!("{e}"))?
        );
        return Ok(());
    }
    let attest_elf = arg("--attest-elf").unwrap_or_else(|| guest_path("attest_program", "attest"));
    let consumer_elf = arg("--consumer-elf").unwrap_or_else(|| guest_path("consumer_erc20_program", "consumer_erc20"));

    let attest_program = AccountId::new([0xA7; 32]);
    let consumer_program = AccountId::new([0xC2; 32]);
    let instance = AccountId::new([0x11; 32]);
    let (attest_bin, attest_len) = load_program(&attest_elf)?;
    let (consumer_bin, consumer_len) = load_program(&consumer_elf)?;
    println!("attest ELF {attest_elf} ({attest_len:?} bytes); consumer ELF {consumer_elf} ({consumer_len:?} bytes)");

    // Cases: (label, fixture, claim field, predicate). Claims as a fulu record holds them (no header), plus the gloas
    // form (header included) for one log and one storage claim.
    let log = Fixture::load("mainnet_log_usdt_tx1_log2.json")?;
    let log_big = Fixture::load("mainnet_log_tx374_log254.json")?;
    let st0 = Fixture::load("mainnet_storage_usdt_slot1.json")?;
    let acct = Fixture::load("mainnet_account_usdc.json")?;
    let slog = Fixture::load("sepolia_log_usdc_tx48_log2.json")?;
    let sst = Fixture::load("sepolia_storage_usdc_slot0.json")?;
    let topics = |f: &Fixture| -> Result<Vec<[u8; 32]>> {
        f.0["expected"]["topics"]
            .as_array()
            .context("topics")?
            .iter()
            .map(|t| f.word(t))
            .collect()
    };
    let transfer_pred = |f: &Fixture| -> Result<Predicate> {
        let t = topics(f)?;
        Ok(Predicate::All(vec![
            Predicate::LogTopicEq { i: 0, v: t[0] },
            Predicate::LogTopicEq { i: 2, v: t[2] },
            Predicate::LogDataWordEq {
                offset: 0,
                v: f.amount()?,
            },
        ]))
    };
    let word_pred =
        |f: &Fixture| -> Result<Predicate> { Ok(Predicate::Eq(PredicateValue::U256(f.word(&f.0["expected"])?))) };
    let gloas = |f: &Fixture| -> Result<attest_core::HeaderRecord> {
        let mut r = f.record(1, [3; 32])?;
        r.exec_number = 0;
        r.exec_state_root = [0; 32];
        r.exec_receipts_root = [0; 32];
        Ok(r)
    };
    let cases = vec![
        (
            "attest_log_mainnet_usdt_transfer",
            log.record(MAINNET_RECORD_SLOT, [3; 32])?,
            log.bytes("claim")?,
            transfer_pred(&log)?,
        ),
        (
            "attest_log_mainnet_tx374_log254_255logs",
            log_big.record(MAINNET_RECORD_SLOT, [3; 32])?,
            log_big.bytes("claim")?,
            Predicate::All(vec![]),
        ),
        (
            "attest_storage_mainnet_usdt_slot1",
            st0.record(MAINNET_RECORD_SLOT, [3; 32])?,
            st0.bytes("claim")?,
            word_pred(&st0)?,
        ),
        (
            "attest_account_mainnet_usdc",
            acct.record(MAINNET_RECORD_SLOT, [3; 32])?,
            acct.bytes("claim")?,
            Predicate::All(vec![]),
        ),
        (
            "attest_log_sepolia_gloas_with_header",
            gloas(&slog)?,
            slog.bytes("claim_with_header")?,
            transfer_pred(&slog)?,
        ),
        (
            "attest_storage_sepolia_gloas_with_header",
            gloas(&sst)?,
            sst.bytes("claim_with_header")?,
            word_pred(&sst)?,
        ),
    ];

    let mut rows = Vec::new();
    for (label, record, claim, predicate) in cases {
        let native = attest_inclusion::verify_claim(instance, &record, &claim, &predicate)
            .map_err(|e| anyhow!("{label}: native verify_claim: {e}"))?;
        let msg = AttestMessage::Attest {
            instance,
            record: record.clone(),
            claim: claim.clone(),
            predicate,
            deliver_to: Actor::new(AccountId::new([0x22; 32]), consumer_program),
        };
        let input = ReceiveInput {
            receiver: Actor::new(AccountId::new([0x33; 32]), attest_program),
            from: None,
            is_authorized: true,
            pre_state: ActorState::empty(),
            message: borsh::to_vec(&msg)?,
        };
        let r = execute(&attest_bin, &input)?;
        println!(
            "{label}: claim {} bytes, message {} bytes -> user cycles {} (total {} in {} segments, {} ms){}",
            claim.len(),
            input.message.len(),
            r.user_cycles,
            r.total_cycles,
            r.segments,
            r.ms,
            if r.ok { "" } else { " REFUSED" }
        );
        rows.push(
            json!({"case": label, "program": "attest", "message": "Attest", "claim_bytes": claim.len(),
            "message_bytes": input.message.len(), "user_cycles": r.user_cycles, "total_cycles": r.total_cycles,
            "segments": r.segments, "exec_ms": r.ms, "accepted": r.ok, "statement_id": hex::encode(native.id)}),
        );

        // Consumer leg, for the transfer log only.
        if label == "attest_log_mainnet_usdt_transfer" {
            let cfg = log.consumer_config(attest_program, instance, 0)?;
            let me = consumer_instance_id(&consumer_program, &cfg);
            let state = ConsumerState::new(cfg);
            let input = ReceiveInput {
                receiver: Actor::new(me, consumer_program),
                from: Some(Actor::new(AccountId::new([0x33; 32]), attest_program)),
                is_authorized: false,
                pre_state: ActorState::from(borsh::to_vec(&state)?),
                message: borsh::to_vec(&native)?,
            };
            let r = execute(&consumer_bin, &input)?;
            println!(
                "consumer_erc20 Statement: user cycles {} ({} ms){}",
                r.user_cycles,
                r.ms,
                if r.ok { "" } else { " REFUSED" }
            );
            rows.push(
                json!({"case": "consumer_erc20_statement", "program": "consumer_erc20", "message": "Statement",
                "user_cycles": r.user_cycles, "total_cycles": r.total_cycles, "segments": r.segments, "exec_ms": r.ms,
                "accepted": r.ok}),
            );
            let null_input = ReceiveInput {
                receiver: Actor::new(
                    consumer_erc20_program::nullifier_id(&consumer_program, &me, &native.id),
                    consumer_program,
                ),
                from: Some(Actor::new(me, consumer_program)),
                is_authorized: true,
                pre_state: ActorState::empty(),
                message: borsh::to_vec(&consumer_erc20_program::ConsumerMessage::MarkConsumed {
                    consumer_instance: me,
                    statement_id: native.id,
                })?,
            };
            let r = execute(&consumer_bin, &null_input)?;
            println!(
                "consumer_erc20 MarkConsumed: user cycles {} ({} ms){}",
                r.user_cycles,
                r.ms,
                if r.ok { "" } else { " REFUSED" }
            );
            rows.push(
                json!({"case": "consumer_erc20_mark_consumed", "program": "consumer_erc20", "message": "MarkConsumed",
                "user_cycles": r.user_cycles, "total_cycles": r.total_cycles, "segments": r.segments, "exec_ms": r.ms,
                "accepted": r.ok}),
            );
        }
    }

    let out = json!({
        "tool": "e2e_consumer/attest_exec",
        "risc0_dev_mode": std::env::var("RISC0_DEV_MODE").ok(),
        "attest_elf": attest_elf, "attest_elf_bytes": attest_len,
        "consumer_elf": consumer_elf, "consumer_elf_bytes": consumer_len,
        "rows": rows,
    });
    if let Some(p) = arg("--json") {
        std::fs::write(&p, serde_json::to_string_pretty(&out)? + "\n")?;
        println!("wrote {p}");
    }
    Ok(())
}
