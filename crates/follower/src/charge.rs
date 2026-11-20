//! What the sequencer charged for a public transaction, read back from its block (LEZ v0.4.0-rc3).
//!
//! rc3 has no per-transaction receipt. A block carries one fee transaction of the builtin `fee` program whose
//! `Distribute` message holds the block's summary (`fee_core::Message`): execution gas used by the whole block. LEZ
//! caps a block at 10M metered cycles and a staged step declares 9M, so a step's block normally holds that step alone
//! among charged transactions; then the block's execution gas is the step's own. When the block holds several charged
//! transactions the per-transaction figure is unknown and only the block total is reported.

use std::str::FromStr;

use anyhow::{Result, anyhow};
use attest_core::AccountId;
use borsh::BorshDeserialize;
use serde::{Deserialize, Serialize};

/// Layout of LEZ rc3 `fee_core::Message` (borsh), the fee program's block message.
#[derive(BorshDeserialize)]
struct BlockFeeSummary {
    gas_used_exec: u64,
    _gas_used_stor: u64,
    _revenue_base: u128,
    _revenue_tip: u128,
}

#[derive(BorshDeserialize)]
enum FeeMessage {
    Distribute {
        summary: BlockFeeSummary,
        _payout: u128,
        _producer: AccountId,
    },
    Refund {
        _amount: u128,
        _payer: AccountId,
    },
}

/// One transaction's block, as the sequencer reports it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Charge {
    pub block: u64,
    /// The block reverted this transaction (it still paid its whole declared gas).
    pub reverted: bool,
    /// Transactions in the block that paid a fee (this one included).
    pub block_charged_txs: usize,
    /// Execution gas the block used, from its fee summary.
    pub block_gas_exec: Option<u64>,
    /// This transaction's execution gas (metered cycles): the block's, when it is the block's only charged
    /// transaction; otherwise unknown.
    pub gas_exec: Option<u64>,
}

/// Reads `hash`'s block from the sequencer the wallet points at.
pub async fn charge_of(hash: &str) -> Result<Charge> {
    let w = attest_host::open_wallet()?;
    let h = common::HashType::from_str(hash).map_err(|e| anyhow!("tx hash {hash}: {e:?}"))?;
    let (_, block_id) = w.poll_transaction(h).await.map_err(|e| anyhow!("tx {hash}: {e:#}"))?;
    let block = w
        .get_block(block_id)
        .await?
        .ok_or_else(|| anyhow!("block {block_id} not found"))?;
    let position = block
        .body
        .transactions
        .iter()
        .position(|t| t.hash() == h)
        .ok_or_else(|| anyhow!("tx {hash} not in block {block_id}"))?;
    let fee_program = AccountId::from_builtin_program_name(b"fee");
    let mut charged = 0usize;
    let mut block_gas = None;
    for t in &block.body.transactions {
        match t {
            common::transaction::LeeTransaction::Public(p) => {
                let ex = &p.message.execution;
                if ex.fee.is_some() {
                    charged += 1;
                } else if ex.root.to.program_account_id == fee_program
                    && let Ok(FeeMessage::Distribute { summary, .. }) = borsh::from_slice(&ex.root.message)
                {
                    block_gas = Some(summary.gas_used_exec);
                }
            }
            common::transaction::LeeTransaction::PrivacyPreserving(_) => charged += 1,
        }
    }
    Ok(Charge {
        block: block_id,
        reverted: block.body.is_reverted(position),
        block_charged_txs: charged,
        block_gas_exec: block_gas,
        gas_exec: if charged == 1 { block_gas } else { None },
    })
}
