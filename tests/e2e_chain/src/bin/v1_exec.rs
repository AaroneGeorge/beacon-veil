//! V1 harness (accept point (a), kill criteria K1/K2): runs the real mainnet light-client chain natively through
//! `attest_core::lc` (Helios 0.12.0 unmodified) and the `lc_verify` guest in the RISC Zero executor, and prints
//! cycles and ELF sizes.
//!
//! usage: v1_exec [--elf PATH] [--all] [--json OUT]
//!   --elf  user ELF of lc_verify (default: the per-guest target dir of crates/guest_methods) or a `.bin`
//!   --all  run every step of the chain in the executor (default: only the final finality update)
//!   --json write the measured numbers to OUT
//!   --init-elf PATH  also run `attest` `Init` on the bootstrap in the executor (user ELF of attest)
//!
//! The executor is the external `r0vm` 3.0.5 (`risc0-zkvm` client), the same one LEZ uses; no proof is made.

use std::time::Instant;

use anyhow::{Context, Result, anyhow, bail};
use attest_core::{AccountId, AttestMessage, LcVerifyMessage, instance_id, lc};
use lee_core::{
    account::{Actor, ActorState},
    program::{ReceiveInput, Transition},
};
use risc0_zkvm::{ExecutorEnv, default_executor};

const CAP_BYTES: usize = 20 * 96 * 1024;

fn default_elf() -> String {
    let target = std::env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| "target".into());
    format!("{target}/riscv-guest/guest_methods/lc_verify_program/riscv32im-risc0-zkvm-elf/release/lc_verify")
}

struct Step {
    label: String,
    witness: lc::StoreWitness,
    update: Vec<u8>,
    out: lc::UpdateOutcome,
    native_ms: u128,
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let arg = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let all = args.iter().any(|a| a == "--all");
    let elf_path = arg("--elf").unwrap_or_else(default_elf);

    // Placeholder ids: the executor run does not touch chain state.
    let attest_program = AccountId::new([0xA7; 32]);
    let lc_verify_program = AccountId::new([0x1C; 32]);
    let chain = e2e_chain::mainnet_chain(vec![lc_verify_program])?;
    let instance = instance_id(&attest_program, &chain.config);
    println!(
        "config: chain_id {} checkpoint slot {} root 0x{} forks {}",
        chain.config.chain_id,
        chain.config.checkpoint_slot,
        hex::encode(chain.config.checkpoint_root),
        chain.config.forks.len()
    );

    // ---- native ----
    let t = Instant::now();
    let (mut witness, head) = lc::init_store(&chain.config, &chain.bootstrap).map_err(|e| anyhow!("init: {e}"))?;
    println!(
        "native init: slot {} period {} store 0x{} ({} ms)",
        head.slot,
        head.period,
        hex::encode(head.store_commitment),
        t.elapsed().as_millis()
    );
    let mut steps = Vec::new();
    for (label, update) in &chain.updates {
        let t = Instant::now();
        let out = lc::verify_and_apply(&chain.config, &witness, update).map_err(|e| anyhow!("{label}: {e}"))?;
        let native_ms = t.elapsed().as_millis();
        println!(
            "native {label}: {} bytes, sig slot {} participation {} -> head slot {} period {} new_slot {} handoff {} \
             exec #{} ({native_ms} ms)",
            update.len(),
            out.signature_slot,
            out.participation,
            out.head.slot,
            out.head.period,
            out.new_slot,
            out.handoff,
            out.head.exec_number
        );
        let next = out.new_witness();
        steps.push(Step {
            label: label.clone(),
            witness: witness.clone(),
            update: update.clone(),
            out,
            native_ms,
        });
        witness = next;
    }
    // Replay of the last update against the new store must be refused.
    let last = steps.last().ok_or_else(|| anyhow!("no steps"))?;
    match lc::verify_and_apply(&chain.config, &witness, &last.update) {
        Err(e) => println!("native replay of {}: refused {e}", last.label),
        Ok(_) => bail!("replay accepted"),
    }

    // ---- attest Init in the executor (must fit the 10M public budget) ----
    if let Some(init_elf) = arg("--init-elf") {
        let raw = std::fs::read(&init_elf).with_context(|| format!("reading {init_elf}"))?;
        let program = risc0_binfmt::ProgramBinary::new(&raw, risc0_zkos_v1compat::V1COMPAT_ELF).encode();
        let input = ReceiveInput {
            receiver: Actor::new(instance, attest_program),
            from: None,
            is_authorized: false,
            pre_state: ActorState::empty(),
            message: borsh::to_vec(&AttestMessage::Init {
                config: chain.config.clone(),
                bootstrap: chain.bootstrap.clone(),
            })?,
        };
        let frame = lee_core::to_frame(&borsh::to_vec(&input)?);
        let env = ExecutorEnv::builder().write_slice(&frame).build()?;
        let info = default_executor().execute(env, &program)?;
        println!(
            "executor attest Init: user cycles {} (public cap 10,000,000), exit {:?}; attest user ELF {} bytes",
            info.cycles(),
            info.exit_code,
            raw.len()
        );
    }

    // ---- executor ----
    let raw = std::fs::read(&elf_path).with_context(|| format!("reading {elf_path}"))?;
    let (user_elf_len, program) = if elf_path.ends_with(".bin") {
        (None, raw.clone())
    } else {
        (
            Some(raw.len()),
            risc0_binfmt::ProgramBinary::new(&raw, risc0_zkos_v1compat::V1COMPAT_ELF).encode(),
        )
    };
    let image_id = risc0_binfmt::compute_image_id(&program).map_err(|e| anyhow!("image id: {e}"))?;
    println!("elf: {elf_path}");
    if let Some(n) = user_elf_len {
        println!(
            "lc_verify user ELF (deployed bytes): {n} bytes = {} segments of 96 KiB; cap {CAP_BYTES}: {}",
            n.div_ceil(96 * 1024),
            if n <= CAP_BYTES { "UNDER" } else { "OVER" }
        );
    }
    println!(
        "lc_verify program binary (with kernel): {} bytes; image id {image_id}",
        program.len()
    );

    let mut results = Vec::new();
    let chosen: Vec<&Step> = if all {
        steps.iter().collect()
    } else {
        vec![steps.last().expect("steps")]
    };
    for s in chosen {
        let msg = LcVerifyMessage::Verify {
            attest_program,
            instance,
            config: chain.config.clone(),
            store_witness: borsh::to_vec(&s.witness)?,
            update_ssz: s.update.clone(),
        };
        let input = ReceiveInput {
            receiver: Actor::new(AccountId::new([0x55; 32]), lc_verify_program),
            from: None,
            is_authorized: true,
            pre_state: ActorState::empty(),
            message: borsh::to_vec(&msg)?,
        };
        let frame = lee_core::to_frame(&borsh::to_vec(&input)?);
        let env = ExecutorEnv::builder().write_slice(&frame).build()?;
        let t = Instant::now();
        let info = default_executor().execute(env, &program)?;
        let exec_ms = t.elapsed().as_millis();
        let user_cycles = info.cycles();
        let total_cycles: u64 = info.segments.iter().map(|seg| 1u64 << seg.po2).sum();
        let journal = lee_core::from_frame(&info.journal.bytes).ok_or_else(|| anyhow!("journal frame"))?;
        let tr: Transition = borsh::from_slice(journal)?;
        let sm = tr
            .response
            .sync_messages
            .first()
            .ok_or_else(|| anyhow!("no sync message"))?;
        let apply: AttestMessage = borsh::from_slice(&sm.message)?;
        let AttestMessage::Apply {
            pre_store,
            post_store,
            head,
            record,
        } = apply
        else {
            bail!("not Apply")
        };
        let same = pre_store == s.out.pre_store
            && post_store == s.out.post_store
            && head == s.out.head
            && record == s.out.record
            && sm.to == Actor::new(instance, attest_program);
        if !same {
            bail!("{}: executor Apply differs from native", s.label);
        }
        println!(
            "executor {}: user cycles {user_cycles}, padded total {total_cycles}, segments {}, exit {:?}, \
             {exec_ms} ms; Apply matches native (head slot {}, period {}, exec #{} 0x{})",
            s.label,
            info.segments.len(),
            info.exit_code,
            head.slot,
            head.period,
            head.exec_number,
            hex::encode(head.exec_hash)
        );
        results.push(serde_json::json!({
            "step": s.label, "update_bytes": s.update.len(), "participation": s.out.participation,
            "handoff": s.out.handoff, "new_slot": s.out.new_slot, "head_slot": head.slot, "period": head.period,
            "user_cycles": user_cycles, "total_cycles_padded": total_cycles, "segments": info.segments.len(),
            "executor_ms": exec_ms, "native_ms": s.native_ms,
        }));
    }
    if let Some(out) = arg("--json") {
        let doc = serde_json::json!({
            "what": "lc_verify V1 (Helios 0.12.0 unmodified) on real mainnet fixtures, RISC Zero 3.0.5 executor",
            "measured": true,
            "elf_path": elf_path, "user_elf_bytes": user_elf_len, "program_binary_bytes": program.len(),
            "cap_bytes": CAP_BYTES, "image_id": image_id.to_string(), "steps": results,
        });
        std::fs::write(&out, serde_json::to_string_pretty(&doc)?)?;
        println!("wrote {out}");
    }
    Ok(())
}
