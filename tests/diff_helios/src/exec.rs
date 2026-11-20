//! Executor column (feature `executor`): each case runs through the `lc_verify_v3` guest itself, as one
//! `VerifyOnce` message in the RISC Zero 3.0.5 executor (`r0vm`, no proof), framed as LEZ frames an actor input. The
//! verdict is read the way LEZ reads it: exit code 0 with a journal whose sync message is `attest.Apply` (accept), or
//! exit code = the `EsaError` number and no journal (refusal).
//!
//! The run is slow (one guest execution of the whole update per case), so it is a separate binary (`diff_exec`) that
//! writes `evidence/diff/executor.json`; the report run reads that file and compares every entry with V1 by input
//! hash, so a stale entry (inputs changed since) is not counted.

use std::path::Path;

use anyhow::{Context, Result, anyhow};
use attest_core::{
    AccountId, AttestMessage, EsaError, InstanceConfig, LcVerifyV3Message, instance_id, lc::StoreWitness,
};
use lee_core::{
    account::{Actor, ActorState},
    program::{ReceiveInput, Transition},
};
use risc0_zkvm::{ExecutorEnv, ExitCode, default_executor};
use sha2::{Digest, Sha256};

use crate::{Accepted, Column, Verdict, v3_inputs};

/// Placeholder ids (the executor touches no chain state); the instance is derived from them as on chain, so the
/// program's instance check (E09) passes exactly when it would on chain.
const ATTEST: AccountId = AccountId::new([0xA7; 32]);
const PROGRAM: AccountId = AccountId::new([0x33; 32]);
const RECEIVER: AccountId = AccountId::new([0x55; 32]);

/// The `lc_verify_v3` guest, loaded once.
pub struct ExecV3 {
    pub elf_path: String,
    pub elf_sha256: String,
    pub image_id: String,
    program: Vec<u8>,
}

/// One executor result.
pub struct ExecResult {
    pub verdict: Verdict,
    pub cycles: u64,
    /// Segments of the execution (the executor's default segment size, 2^20 cycles, as LEZ proves).
    pub segments: usize,
}

impl ExecV3 {
    /// Loads a user ELF (wrapped as a v1compat program binary) or a `.bin` program binary.
    pub fn load(path: &Path) -> Result<Self> {
        let raw = std::fs::read(path).with_context(|| path.display().to_string())?;
        let program = if path.extension().is_some_and(|e| e == "bin") {
            raw.clone()
        } else {
            risc0_binfmt::ProgramBinary::new(&raw, risc0_zkos_v1compat::V1COMPAT_ELF).encode()
        };
        let image_id = risc0_binfmt::compute_image_id(&program).map_err(|e| anyhow!("image id: {e}"))?;
        Ok(Self {
            elf_path: path.display().to_string(),
            elf_sha256: hex::encode(Sha256::digest(&raw)),
            image_id: image_id.to_string(),
            program,
        })
    }

    /// Runs one case; executor errors (a guest panic or trap) become `Verdict::Panic`.
    pub fn execute(&self, config: &InstanceConfig, witness: &StoreWitness, update_ssz: &[u8]) -> ExecResult {
        match self.try_execute(config, witness, update_ssz) {
            Ok(r) => r,
            Err(e) => ExecResult {
                verdict: Verdict::Panic(format!("{e:#}")),
                cycles: 0,
                segments: 0,
            },
        }
    }

    fn try_execute(&self, config: &InstanceConfig, witness: &StoreWitness, update_ssz: &[u8]) -> Result<ExecResult> {
        let (w3, absent_keys) = v3_inputs(config, witness, update_ssz);
        let instance = instance_id(&ATTEST, config);
        let msg = LcVerifyV3Message::VerifyOnce {
            attest_program: ATTEST,
            instance,
            config: config.clone(),
            witness: w3,
            update_ssz: update_ssz.to_vec(),
            absent_keys,
        };
        let input = ReceiveInput {
            receiver: Actor::new(RECEIVER, PROGRAM),
            from: None,
            is_authorized: true,
            pre_state: ActorState::empty(),
            message: borsh::to_vec(&msg)?,
        };
        let frame = lee_core::to_frame(&borsh::to_vec(&input)?);
        let mut stderr = Vec::new();
        let info = {
            let env = ExecutorEnv::builder().write_slice(&frame).stderr(&mut stderr).build()?;
            default_executor().execute(env, &self.program)?
        };
        let cycles = info.cycles();
        let segments = info.segments.len();
        let verdict = match info.exit_code {
            ExitCode::Halted(0) => {
                let journal = lee_core::from_frame(&info.journal.bytes).ok_or_else(|| anyhow!("journal frame"))?;
                let t: Transition = borsh::from_slice(journal).context("transition")?;
                let [sm] = t.response.sync_messages.as_slice() else {
                    return Err(anyhow!("{} sync messages, want 1", t.response.sync_messages.len()));
                };
                if sm.to != Actor::new(instance, ATTEST) {
                    return Err(anyhow!("Apply sent to the wrong actor"));
                }
                match borsh::from_slice::<AttestMessage>(&sm.message)? {
                    AttestMessage::Apply {
                        pre_store,
                        post_store,
                        head,
                        record,
                    } => Verdict::Accept(Box::new(Accepted {
                        pre_store,
                        post_store,
                        head,
                        record,
                    })),
                    _ => return Err(anyhow!("sync message is not Apply")),
                }
            }
            ExitCode::Halted(n) => EsaError::ALL
                .into_iter()
                .find(|e| u32::from(*e as u8) == n)
                .map_or_else(|| Verdict::Panic(format!("exit code {n}")), Verdict::Refuse),
            other => Verdict::Panic(format!(
                "exit {other:?}: {}",
                String::from_utf8_lossy(&stderr).lines().last().unwrap_or_default()
            )),
        };
        Ok(ExecResult {
            verdict,
            cycles,
            segments,
        })
    }
}

impl Column for ExecV3 {
    fn name(&self) -> &'static str {
        "v3_exec"
    }
    fn verify(&self, config: &InstanceConfig, witness: &StoreWitness, update_ssz: &[u8]) -> Verdict {
        self.execute(config, witness, update_ssz).verdict
    }
}
