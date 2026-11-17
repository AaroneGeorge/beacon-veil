//! `hello_public <hello_world.bin> <account_id> <payer_account_id> [<program_header_id>]`
//!
//! Acceptance smoke test on a local rc3 sequencer. Same steps as LEZ's `run_hello_world` example, except that the
//! transaction goes through `WalletCore::send_pub_tx_paid_by`, which adds the fee declaration rc3's sequencer requires
//! (the rc3 example sends a bare `PublicTransaction` and is rejected with "transaction fee classification failed").
//! If `program_header_id` is given, the already-deployed program is reused instead of deploying the binary again.

use anyhow::Context as _;
use lee::{AccountId, program::Program};
use program_loader_core::MAX_SEGMENT_DATA_LEN;
use wallet::{AccountIdentity, WalletCore, program_facades::program_loader::ProgramLoader};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    anyhow::ensure!(
        args.len() == 4 || args.len() == 5,
        "usage: hello_public <hello_world.bin> <account_id> <payer_account_id> [<program_header_id>]"
    );
    let bytecode = std::fs::read(&args[1]).with_context(|| format!("read {}", args[1]))?;
    let account: AccountId = args[2].parse().context("account id")?;
    let payer: AccountId = args[3].parse().context("payer id")?;
    let mut wallet_core = WalletCore::from_env()?;

    let program = if let Some(h) = args.get(4) {
        h.parse().context("program header id")?
    } else {
        let user_len = risc0_user_len(&bytecode)?;
        let n = user_len.div_ceil(MAX_SEGMENT_DATA_LEN);
        let header = wallet_core.create_new_account_public(None).0;
        let segments: Vec<AccountId> = std::iter::repeat_with(|| wallet_core.create_new_account_public(None).0)
            .take(n)
            .collect();
        wallet_core.store_persistent_data()?;
        ProgramLoader(&wallet_core)
            .deploy(header, &segments, bytecode, true, Some(payer))
            .await?
    };
    println!("program header {program}");

    let greeting: Vec<u8> = b"Hola mundo!".to_vec();
    let mention = AccountIdentity::Public(account).select_program_actor_state(program);
    let tx = wallet_core
        .send_pub_tx_paid_by(vec![mention], 0, Program::serialize_message(greeting)?, Some(payer))
        .await
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    println!("hello tx {tx}");
    Ok(())
}

/// User-ELF length out of a risc0 `ProgramBinary` ("R0BF" | u32 version | u32 header_len | header | u32 user_len).
fn risc0_user_len(bin: &[u8]) -> anyhow::Result<usize> {
    let u32_at = |o: usize| -> anyhow::Result<usize> {
        let b: [u8; 4] = bin.get(o..o + 4).context("short ProgramBinary")?.try_into()?;
        Ok(u32::from_le_bytes(b) as usize)
    };
    anyhow::ensure!(bin.get(..4) == Some(b"R0BF".as_slice()), "not a ProgramBinary");
    let header_len = u32_at(8)?;
    u32_at(12 + header_len)
}
