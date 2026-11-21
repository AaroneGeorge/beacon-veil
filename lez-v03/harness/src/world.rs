//! An in-process LEZ v0.3.2 public state with both programs deployed, executing transactions as the sequencer's
//! action phase does.

use std::borrow::Cow;

use anyhow::{Result, anyhow};
use lee::{
    PublicTransaction, V03State, ValidatedStateDiff,
    program::Program,
    public_transaction::{Message, WitnessSet},
};
use lee_core::account::{AccountId, ShardData};

use crate::steps::{Ids, Tx};

/// The block gas cap of LEZ testnet 0.3 (`getFeeState`: `max_gas_exec`), the most one transaction may declare.
pub const BLOCK_CAP: u64 = 10_000_000;
/// The most a step may cost in this port (90% of the cap), and the most the client declares.
pub const STEP_LIMIT: u64 = 9_000_000;

/// Result of one executed transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// Cycles the action phase meters (what a charged transaction pays for, before the `gas_limit` clamp).
    pub cycles: u64,
    /// `Ok` when the transaction applied; otherwise the refusal (the state is unchanged).
    pub result: Result<(), String>,
    /// For a refusal by exit code: the code.
    pub exit_code: Option<u32>,
}

pub struct World {
    pub state: V03State,
    pub ids: Ids,
    pub block_id: u64,
}

/// Reads a `.bin` (`ProgramBinary`) and returns the program and its image id.
pub fn program(bin: &[u8]) -> Result<Program> {
    Program::new(Cow::Owned(bin.to_vec())).map_err(|e| anyhow!("program: {e}"))
}

impl World {
    /// Both programs at `ids`, deployed as the program loader lays out an immutable program (header and segments).
    pub fn new(ids: Ids, attest_bin: &[u8], v3_bin: &[u8]) -> Result<Self> {
        let state =
            V03State::new().with_named_programs([(ids.attest, program(attest_bin)?), (ids.v3, program(v3_bin)?)]);
        Ok(Self {
            state,
            ids,
            block_id: 1,
        })
    }

    /// The data of `program`'s shard on `account` (empty when absent).
    #[must_use]
    pub fn shard(&self, account: AccountId, program: AccountId) -> Vec<u8> {
        self.state
            .get_account_by_id(account)
            .data
            .shard(program)
            .clone()
            .into_inner()
    }

    /// Executes `tx` (no signer, fee-exempt: the action phase alone) at `timestamp_ms` with gas limit `budget`, and
    /// applies it if it succeeds.
    pub fn exec(&mut self, tx: &Tx, timestamp_ms: u64, budget: u64) -> Outcome {
        let message =
            Message::new_preserialized(tx.program, tx.selectors.clone(), vec![], tx.instruction.clone(), None);
        let witness = WitnessSet::for_message(&message, &[]);
        let ptx = PublicTransaction::new(message, witness);
        self.exec_public(&ptx, timestamp_ms, budget)
    }

    /// Executes a built public transaction (signed or not) and applies it if it succeeds.
    pub fn exec_public(&mut self, ptx: &PublicTransaction, timestamp_ms: u64, budget: u64) -> Outcome {
        self.block_id += 1;
        match ValidatedStateDiff::from_public_transaction_with_cycle_budget(
            ptx,
            &self.state,
            self.block_id,
            timestamp_ms,
            budget,
        ) {
            Ok((diff, charge)) => {
                drop(self.state.apply_state_diff(diff));
                Outcome {
                    cycles: charge.cycles,
                    result: Ok(()),
                    exit_code: None,
                }
            }
            Err(e) => {
                // A refusal: run again, metered, for the cycles the action phase charges it (a non-zero exit is
                // charged its cycles, a panic or out-of-gas the whole budget).
                let (charge, _) = ValidatedStateDiff::from_public_transaction_metered(
                    ptx,
                    &self.state,
                    self.block_id,
                    timestamp_ms,
                    budget,
                );
                Outcome {
                    cycles: charge.cycles,
                    exit_code: match &e {
                        lee::error::LeeError::ProgramExitedWithCode { code, .. } => Some(*code),
                        _ => None,
                    },
                    result: Err(e.to_string()),
                }
            }
        }
    }
}

/// `ShardData` from bytes (tests).
pub fn shard_data(bytes: Vec<u8>) -> Result<ShardData> {
    ShardData::try_from(bytes).map_err(|e| anyhow!("shard data: {e:?}"))
}
