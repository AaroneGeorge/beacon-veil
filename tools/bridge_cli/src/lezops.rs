//! LEZ side of bridge_cli, on `attest_host`: wallet, public and private transactions, record reads.

use std::path::PathBuf;

use anyhow::{Context as _, Result, anyhow, bail};
use attest_core::{AccountId, Actor, Predicate, PredicateValue};
use attest_host::{
    DeployedIds,
    lee::privacy_preserving_transaction::circuit::ProgramCatalog,
    wallet::{AccountIdentity, AccountMention},
};
use bridge_program::{
    Addr, B32,
    hash::{burn_leaf, burn_note, burn_secret, deposit_nullifier, memo_open},
    journal::BurnWitness,
    lez::{
        Accumulator, BridgeError, BridgeMessage, BurnEvent, ClaimArgs, accumulator_seed, leaf_record_seed,
        nullifier_seed, pda, root_seed,
    },
    tree::path_for,
};
use lee_core::program::PdaSeed;
use serde_json::{Value, json};

use crate::{
    ConfigFile,
    eth::{Rpc, quantity},
    hex0x,
};

fn mention_pda(program: AccountId, seed: &PdaSeed) -> AccountMention {
    attest_host::pda(program, *seed.as_bytes()).select_program_actor_state(program)
}

async fn state_of(w: &attest_host::Wallet, account: AccountId, program: AccountId) -> Result<Vec<u8>> {
    Ok(w.get_account_view(Actor::new(account, program))
        .await?
        .map(|a| a.data.actor_state(program).to_vec())
        .unwrap_or_default())
}

fn bridge_programs(cf: &ConfigFile) -> Result<ProgramCatalog> {
    let dir = attest_host::guests_dir();
    Ok(ProgramCatalog::from([(
        cf.bridge_program,
        attest_host::program_from(&dir, "bridge")?,
    )]))
}

/// Public: proves `vault.depositRoot` at the record of `slot` and delivers the statement to the root receipt PDA.
/// The root is public on Ethereum and shared by every deposit before it, so this step reveals no deposit.
pub async fn attest_root(
    cf: &ConfigFile,
    rpc: &Rpc,
    slot: Option<u64>,
    min_eth_block: u64,
    proofgen: Option<PathBuf>,
) -> Result<()> {
    let ids = DeployedIds::from_env()?;
    if ids.attest != cf.config.attest_program {
        bail!(
            "deploy ids attest program {} differs from the bridge config's {}",
            ids.attest,
            cf.config.attest_program
        );
    }
    let slot = match slot {
        Some(s) => s,
        None => {
            attest_host::read_head(&ids, &cf.config.attest_instance)
                .await?
                .context("attest instance not found on this sequencer")?
                .slot
        }
    };
    let record = attest_host::read_record(&ids, &cf.config.attest_instance, slot)
        .await?
        .with_context(|| format!("no header record at slot {slot}"))?;
    if record.chain_id != cf.config.chain_id {
        bail!(
            "record chain {} != bridge chain {}",
            record.chain_id,
            cf.config.chain_id
        );
    }
    // gloas records carry only exec_hash (exec_number and roots zero): find the number by hash.
    let number = if record.exec_number != 0 {
        record.exec_number
    } else {
        quantity(&rpc.block_by_hash(&record.exec_hash).await?["number"])?
    };
    if number < min_eth_block {
        bail!(
            "{}: record slot {slot} proves execution block {number}, before block {min_eth_block}; wait for the follower",
            BridgeError::RootNotAttested.describe()
        );
    }
    let fixture = run_proofgen(cf, rpc, number, proofgen)?;
    let block_hash = crate::eth::hex32(&fixture["block_hash"])?;
    if block_hash != record.exec_hash {
        bail!(
            "proofgen block hash {} != record exec_hash {}",
            hex0x(&block_hash),
            hex0x(&record.exec_hash)
        );
    }
    let gloas = record.exec_state_root == [0; 32];
    let claim_hex = fixture[if gloas { "claim_with_header" } else { "claim" }]
        .as_str()
        .context("claim")?;
    let claim = hex::decode(claim_hex.trim_start_matches("0x"))?;
    let root = crate::eth::hex32(&fixture["proven_word"])?;
    if root == [0; 32] {
        bail!("vault root slot is empty at block {number}: wrong vault or block before deployment");
    }
    let stmt_id = cf.config.root_statement_id(&root);
    let rseed = root_seed(&cf.config.attest_program, &cf.config.attest_instance, &stmt_id);
    let receipt = pda(&cf.bridge_program, &rseed);
    let w = attest_host::open_wallet()?;
    let existing = state_of(&w, receipt, cf.bridge_program).await?;
    if !existing.is_empty() {
        println!(
            "{}",
            json!({"root": hex0x(&root), "receipt": receipt.to_string(), "already_attested": true})
        );
        return Ok(());
    }
    let (statement, out) = attest_host::attest_with(
        &ids,
        &cf.config.attest_instance,
        &record,
        &claim,
        &Predicate::Eq(PredicateValue::Bytes32(root)),
        &Actor::new(receipt, cf.bridge_program),
        Some(mention_pda(cf.bridge_program, &rseed)),
        Vec::new(),
    )
    .await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "root": hex0x(&root),
            "receipt": receipt.to_string(),
            "statement_id": hex0x(&statement.id),
            "record_slot": slot,
            "eth_block": number,
            "tx": out.tx,
            "gas": out.gas,
            "public_on_lez": "the vault root and which header record proved it; no deposit is named",
        }))?
    );
    Ok(())
}

/// DEV ONLY: see `programs/bridge/src/bin/bridge_dev_source.rs`.
pub async fn dev_attest_root(cf: &ConfigFile, rpc: &Rpc) -> Result<()> {
    if cf.config.chain_id != 31_337 {
        bail!("dev-attest-root is for a local anvil (chain 31337) only");
    }
    let raw = rpc
        .call(
            "eth_getStorageAt",
            json!([
                hex0x(&cf.config.vault),
                hex0x(&bridge_program::DEPOSIT_ROOT_SLOT),
                "latest"
            ]),
        )
        .await?;
    let root = crate::eth::hex32(&raw)?;
    let mut subject = cf.config.vault.to_vec();
    subject.extend_from_slice(&bridge_program::DEPOSIT_ROOT_SLOT);
    let statement = attest_core::Statement {
        instance: cf.config.attest_instance,
        chain_id: cf.config.chain_id,
        slot: 0,
        exec_number: rpc.block_number().await?,
        kind: attest_core::ClaimKind::Storage,
        subject,
        value: root.to_vec(),
        predicate_hash: Predicate::Eq(PredicateValue::Bytes32(root)).hash(),
        id: cf.config.root_statement_id(&root),
    };
    let rseed = root_seed(&cf.config.attest_program, &cf.config.attest_instance, &statement.id);
    let receipt = pda(&cf.bridge_program, &rseed);
    let source = cf.config.attest_program;
    let mut w = attest_host::open_wallet()?;
    let payer = attest_host::account_from_env_or_label(&w, "BV_PAYER", "bv-payer")?;
    let accounts = vec![
        AccountIdentity::Public(payer).select_program_actor_state(source),
        mention_pda(cf.bridge_program, &rseed),
    ];
    let msg = borsh::to_vec(&(Actor::new(receipt, cf.bridge_program), statement))?;
    let out = attest_host::send_public(&mut w, accounts, msg, "dev attest root").await?;
    println!(
        "{}",
        json!({"root": hex0x(&root), "receipt": receipt.to_string(), "tx": out.tx, "dev_only": true})
    );
    Ok(())
}

/// Read only: head of the configured attest instance, and the execution block number its record proves.
pub async fn instance_head(cf: &ConfigFile, rpc: &Rpc) -> Result<()> {
    let ids = DeployedIds::from_env()?;
    let head = attest_host::read_head(&ids, &cf.config.attest_instance)
        .await?
        .context("attest instance not found on this sequencer")?;
    let record = attest_host::read_record(&ids, &cf.config.attest_instance, head.slot).await?;
    let hash = record.as_ref().map_or(head.exec_hash, |r| r.exec_hash);
    let number = quantity(&rpc.block_by_hash(&hash).await?["number"])?;
    println!(
        "{}",
        json!({"slot": head.slot, "period": head.period, "exec_hash": hex0x(&hash), "eth_block": number,
               "record_present": record.is_some()})
    );
    Ok(())
}

fn run_proofgen(cf: &ConfigFile, rpc: &Rpc, number: u64, proofgen: Option<PathBuf>) -> Result<Value> {
    let bin = proofgen.unwrap_or_else(|| {
        PathBuf::from(std::env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| "target".into())).join("debug/proofgen")
    });
    let chain = match cf.config.chain_id {
        1 => "mainnet",
        11_155_111 => "sepolia",
        other => bail!("proofgen knows mainnet and sepolia, not chain {other}"),
    };
    let out = std::env::temp_dir().join(format!("bv-proofgen-{}-{number}.json", std::process::id()));
    let status = std::process::Command::new(&bin)
        .args([
            "--chain",
            chain,
            "--rpc",
            &rpc.url,
            "--block",
            &number.to_string(),
            "--out",
        ])
        .arg(&out)
        .args([
            "storage",
            "--address",
            &hex0x(&cf.config.vault),
            "--slot",
            &hex0x(&bridge_program::DEPOSIT_ROOT_SLOT),
        ])
        .status()
        .with_context(|| format!("running {}", bin.display()))?;
    if !status.success() {
        bail!("proofgen failed ({status})");
    }
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&out)?)?;
    let _ = std::fs::remove_file(&out);
    Ok(v)
}

/// Private: one transaction, the claim proven on this machine. Reveals the root receipt it reads and the nullifier
/// PDA it marks; with a private destination nothing else (Privacy #1, #3).
pub async fn claim(cf: &ConfigFile, args: ClaimArgs, claimer: AccountId, public: bool, force: bool) -> Result<()> {
    let bridge = cf.bridge_program;
    let mut w = attest_host::open_wallet()?;
    let stmt_id = cf.config.root_statement_id(&args.root);
    let rseed = root_seed(&cf.config.attest_program, &cf.config.attest_instance, &stmt_id);
    if state_of(&w, pda(&bridge, &rseed), bridge).await?.is_empty() {
        bail!(
            "{} (run `attest-root` for a record at or after the deposit)",
            BridgeError::RootNotAttested.describe()
        );
    }
    let nullifier = deposit_nullifier(&args.secret);
    let nseed = nullifier_seed(&cf.instance, &nullifier);
    if !force && !state_of(&w, pda(&bridge, &nseed), bridge).await?.is_empty() {
        return Err(anyhow::Error::new(attest_core::EsaError::E11AlreadyConsumed).context("claim: already claimed"));
    }
    if public && args.dest == claimer {
        bail!("a public destination needs a separate private --claimer: the claim itself must run privately");
    }
    let mut accounts = vec![
        AccountIdentity::PrivateOwned(claimer).select_program_actor_state(bridge),
        mention_pda(bridge, &rseed),
        mention_pda(bridge, &nseed),
    ];
    if args.dest != claimer {
        accounts.push(if public {
            AccountIdentity::PublicNoSign(args.dest).select_program_actor_state(bridge)
        } else {
            AccountIdentity::PrivateOwned(args.dest).select_program_actor_state(bridge)
        });
    }
    let fee = cf.config.token(&args.token).map_or(0, |t| t.mint_fee);
    if fee > 0
        && let Some(t) = cf.config.treasury
    {
        accounts.push(AccountIdentity::PublicNoSign(t).select_program_actor_state(bridge));
    }
    let (token, denom, dest, root) = (args.token, args.denom, args.dest, args.root);
    let msg = borsh::to_vec(&BridgeMessage::Claim(Box::new(args)))?;
    let started = std::time::Instant::now();
    let out = attest_host::send_private(&mut w, accounts, msg, &bridge_programs(cf)?, "claim").await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "tx": out.tx,
            "seconds": started.elapsed().as_secs_f64(),
            "dev_mode": std::env::var("RISC0_DEV_MODE").is_ok_and(|v| v == "1"),
            "token": hex0x(&token),
            "denom": denom.to_string(),
            "dest": dest.to_string(),
            "dest_kind": if public { "public (explicit)" } else { "private (default)" },
            "root": hex0x(&root),
            "nullifier_pda": pda(&bridge, &nseed).to_string(),
            "public_on_lez": if public {
                "the root used, a fresh nullifier, the credit to the public destination (token, amount)"
            } else {
                "the root used and a fresh nullifier; not the destination, not which deposit"
            },
        }))?
    );
    Ok(())
}

pub async fn read_accumulator(cf: &ConfigFile) -> Result<Accumulator> {
    let w = attest_host::open_wallet()?;
    let s = state_of(
        &w,
        pda(&cf.bridge_program, &accumulator_seed(&cf.instance)),
        cf.bridge_program,
    )
    .await?;
    if s.is_empty() {
        Ok(Accumulator::default())
    } else {
        Ok(borsh::from_slice(&s)?)
    }
}

pub async fn burn_records(cf: &ConfigFile) -> Result<Vec<BurnEvent>> {
    let w = attest_host::open_wallet()?;
    let acc = read_accumulator(cf).await?;
    let mut out = Vec::new();
    for i in 0..acc.tree.count {
        let s = state_of(
            &w,
            pda(&cf.bridge_program, &leaf_record_seed(&cf.instance, i)),
            cf.bridge_program,
        )
        .await?;
        out.push(borsh::from_slice::<BurnEvent>(&s).with_context(|| format!("burn leaf record {i} missing"))?);
    }
    Ok(out)
}

pub async fn is_claimed(cf: &ConfigFile, deposit_nullifier: &B32) -> Result<bool> {
    let w = attest_host::open_wallet()?;
    let seed = nullifier_seed(&cf.instance, deposit_nullifier);
    Ok(!state_of(&w, pda(&cf.bridge_program, &seed), cf.bridge_program)
        .await?
        .is_empty())
}

/// Burn from `holder`. Declares the leaf record of the next index; a concurrent burn makes this one fail whole
/// (nothing debited, R2) and the CLI can be re-run.
pub async fn burn(
    cf: &ConfigFile,
    holder: AccountId,
    public: bool,
    token: Addr,
    denom: u128,
    note: B32,
    memo: [u8; 36],
) -> Result<()> {
    let bridge = cf.bridge_program;
    let entry = cf
        .config
        .token(&token)
        .ok_or_else(|| anyhow!(BridgeError::UnsupportedToken.describe()))?;
    if !entry.denoms.contains(&denom) {
        bail!(BridgeError::InvalidAmount.describe());
    }
    let next = read_accumulator(cf).await?.tree.count;
    let mut accounts = vec![
        if public {
            AccountIdentity::Public(holder).select_program_actor_state(bridge)
        } else {
            AccountIdentity::PrivateOwned(holder).select_program_actor_state(bridge)
        },
        mention_pda(bridge, &accumulator_seed(&cf.instance)),
        mention_pda(bridge, &leaf_record_seed(&cf.instance, next)),
    ];
    if entry.burn_fee > 0
        && let Some(t) = cf.config.treasury
    {
        accounts.push(AccountIdentity::PublicNoSign(t).select_program_actor_state(bridge));
    }
    let msg = borsh::to_vec(&BridgeMessage::Burn {
        config: cf.config.clone(),
        token,
        denom,
        note,
        memo,
    })?;
    let mut w = attest_host::open_wallet()?;
    let out = if public {
        attest_host::send_public(&mut w, accounts, msg, "burn").await?
    } else {
        attest_host::send_private(&mut w, accounts, msg, &bridge_programs(cf)?, "burn").await?
    };
    eprintln!("burn tx {} (leaf index {next})", out.tx);
    Ok(())
}

/// Rebuilds the witness of burn `index` from the seed and the leaf records.
pub async fn burn_witness(cf: &ConfigFile, key: &B32, index: u64) -> Result<BurnWitness> {
    let records = burn_records(cf).await?;
    let acc = read_accumulator(cf).await?;
    let secret = burn_secret(key, cf.instance.value(), index);
    let (rec, recipient, fee) = records
        .iter()
        .find_map(|r| {
            let (recipient, fee) = memo_open(&secret, &r.memo);
            (burn_note(&secret, &recipient, fee) == r.note).then_some((r, recipient, fee))
        })
        .context("no burn of this index in the accumulator")?;
    let leaves: Vec<B32> = records.iter().map(|r| burn_leaf(&r.token, r.denom, &r.note)).collect();
    let (root, path) = path_for(&leaves, usize::try_from(rec.index)?);
    if root != acc.root {
        bail!("rebuilt accumulator root differs from on-chain root");
    }
    Ok(BurnWitness {
        burn_secret: secret,
        recipient,
        fee,
        token: rec.token,
        denom: rec.denom,
        leaf_index: rec.index,
        path,
        chain_id: cf.config.chain_id,
        vault: cf.config.vault,
        lez_program: *cf.bridge_program.value(),
        lez_instance: *cf.instance.value(),
    })
}
