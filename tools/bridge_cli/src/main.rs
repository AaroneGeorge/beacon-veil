//! `bridge_cli`: the user and operator side of the RFP-022 spike.
//!
//! Secrets come only from the LEZ wallet mnemonic (`keys.rs`, F9). Ethereum writes go through `forge script`
//! (contracts/script/Deploy.s.sol), which reads `PRIVATE_KEY` from the environment; this tool never reads or prints a
//! key. LEZ access uses `attest_host` and the wallet at `$LEE_WALLET_HOME_DIR`.
//!
//! Inbound:  `commitment` -> (forge DepositOne) -> `attest-root` -> `claim`.   Outbound: `burn` -> `post-root` (STUB
//! anchor) -> `prove` -> `release`.   Recovery: `recover` lists every unclaimed deposit and unreleased burn of the
//! seed, from chain state only (F9, R5).
//!
//! Defaults follow U5/U6: the claim destination and the burn source default to a private account; a public one needs
//! `--public`, and every command prints what it leaks.

mod eth;
mod keys;
mod lezops;
mod prove;

use std::path::PathBuf;

use anyhow::{Context as _, Result, bail};
use attest_core::AccountId;
use bridge_program::{
    Addr, B32, NATIVE,
    hash::{
        burn_leaf, burn_note, burn_nullifier, burn_secret, commitment, deposit_nullifier, deposit_secret, dest_binding,
        leaf, memo_open, memo_seal,
    },
    lez::{BridgeConfig, TokenEntry, instance_id},
    tree::path_for,
};
use clap::{Parser, Subcommand};
use serde_json::{Value, json};

use crate::eth::{Rpc, parse_addr, parse_b32};

#[derive(Parser)]
#[command(about = "beacon-veil bridge CLI (RFP-022 spike)")]
struct Cli {
    /// Bridge config file (written by `config`).
    #[arg(long, env = "BV_BRIDGE_CONFIG", global = true)]
    config: Option<PathBuf>,
    /// Ethereum JSON-RPC.
    #[arg(
        long,
        env = "BV_ETH_RPC",
        global = true,
        default_value = "https://ethereum-sepolia-rpc.publicnode.com"
    )]
    rpc: String,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Writes a bridge config file and prints the instance id.
    Config {
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        bridge_program: AccountId,
        #[arg(long)]
        chain_id: u64,
        #[arg(long)]
        vault: String,
        #[arg(long, default_value_t = 0)]
        vault_block: u64,
        #[arg(long)]
        anchor: Option<String>,
        #[arg(long)]
        attest_program: AccountId,
        #[arg(long)]
        attest_instance: AccountId,
        /// `TOKEN:DENOM,DENOM[:MINT_FEE:BURN_FEE]`, base units; TOKEN `eth` for native ETH. Repeatable.
        #[arg(long = "token", required = true)]
        tokens: Vec<String>,
        #[arg(long)]
        treasury: Option<AccountId>,
    },
    /// Deposit commitment number `index` for `dest` (no secret printed).
    Commitment {
        #[arg(long)]
        index: u64,
        #[arg(long)]
        dest: AccountId,
        #[arg(long)]
        token: String,
        #[arg(long)]
        denom: u128,
    },
    /// Public: attest `vault.depositRoot` at a finalised record and deliver it to the shared root receipt.
    AttestRoot {
        /// Record slot of the attest instance (finalised header) to prove against. Default: the instance head.
        #[arg(long)]
        slot: Option<u64>,
        /// Fail unless the record's execution block is at or after this Ethereum block (the deposit's block).
        #[arg(long, default_value_t = 0)]
        min_eth_block: u64,
        /// proofgen binary.
        #[arg(long, env = "BV_PROOFGEN")]
        proofgen: Option<PathBuf>,
    },
    /// Private: claim deposit `index` into `dest` against an attested root.
    Claim {
        #[arg(long)]
        index: u64,
        #[arg(long)]
        dest: AccountId,
        /// Root to claim against (default: the latest deposit root whose receipt exists is not searched; give it).
        #[arg(long)]
        root: String,
        /// Private account that runs the claim (default: `dest`).
        #[arg(long)]
        claimer: Option<AccountId>,
        /// `dest` is a public account (explicit choice, U6).
        #[arg(long)]
        public: bool,
        /// Send even if the local pre-check finds the deposit already claimed (to show the chain refusing it).
        #[arg(long)]
        force: bool,
    },
    /// Burn one denomination from `holder` into the accumulator; the Ethereum recipient stays hidden in the note.
    Burn {
        #[arg(long)]
        index: u64,
        #[arg(long)]
        holder: AccountId,
        #[arg(long)]
        token: String,
        #[arg(long)]
        denom: u128,
        #[arg(long)]
        recipient: String,
        /// Relayer fee paid out of the denomination on release, base units.
        #[arg(long, default_value_t = 0)]
        fee: u128,
        /// `holder` is a public account (explicit choice, U6).
        #[arg(long)]
        public: bool,
    },
    /// Prints the burn accumulator root and leaf count (what the STUB anchor owner posts).
    AccumulatorRoot,
    /// Builds the burn_proof witness for burn `index`, checks it natively, and proves it (dev mode or Groth16).
    Prove {
        #[arg(long)]
        index: u64,
        #[arg(long)]
        out: PathBuf,
        /// `none` (native check only), `witness` (write witness.borsh, 0600, for the RISC Zero 3.1.0 prover in
        /// guests/burn_proof/r31), `execute` (3.0.5 executor, cycles), `groth16` (3.0.5 real proof: superseded, see
        /// GHSA-g6qg-9j8p-7j87).
        #[arg(long, default_value = "execute")]
        mode: String,
    },
    /// Lists every own deposit and burn and whether it is still open, from the seed and chain state only.
    Recover {
        /// Candidate destination accounts (the wallet's own accounts).
        #[arg(long = "dest", required = true)]
        dests: Vec<AccountId>,
        #[arg(long, default_value_t = 20)]
        gap: u64,
        /// Also query LEZ for claim and burn state.
        #[arg(long)]
        lez: bool,
    },
    /// DEV ONLY (anvil, chain 31337): deliver "vault root = current root" through the dev statement source, which the
    /// config must name as `attest_program`. Stands in for `attest-root` where no light client follows the chain.
    DevAttestRoot,
    /// Read only: the attest instance's head and the Ethereum block its record proves.
    InstanceHead,
    /// Prints the image id of guest burn_proof (`$BV_GUESTS/burn_proof.bin`).
    ImageId,
    /// Cross-language test vectors for contracts/test/BridgeVectors.t.sol.
    Vectors {
        #[arg(long)]
        out: PathBuf,
    },
}

/// A bridge config file: the borsh config plus what the CLI needs around it.
pub struct ConfigFile {
    pub bridge_program: AccountId,
    pub config: BridgeConfig,
    pub instance: AccountId,
    pub vault_block: u64,
    pub anchor: Option<Addr>,
}

pub fn token_arg(s: &str) -> Result<Addr> {
    if s.eq_ignore_ascii_case("eth") {
        Ok(NATIVE)
    } else {
        parse_addr(s)
    }
}

fn hex0x(b: &[u8]) -> String {
    format!("0x{}", hex::encode(b))
}

impl ConfigFile {
    fn load(path: Option<&PathBuf>) -> Result<Self> {
        let path = path.context("--config (or $BV_BRIDGE_CONFIG) is required")?;
        let v: Value = serde_json::from_str(&std::fs::read_to_string(path)?)?;
        let borsh_hex = v["config_borsh"].as_str().context("config_borsh")?;
        let config: BridgeConfig = borsh::from_slice(&hex::decode(borsh_hex.trim_start_matches("0x"))?)?;
        let bridge_program: AccountId = v["bridge_program"]
            .as_str()
            .context("bridge_program")?
            .parse()
            .map_err(|e| anyhow::anyhow!("{e:?}"))?;
        let instance = instance_id(&bridge_program, &config);
        // The DEV statement source is for a local anvil only: refuse it anywhere else.
        if config.chain_id != 31_337
            && let Ok(ids) = std::fs::read_to_string(attest_host::ids_path())
            && ids.lines().any(|l| {
                l.trim_start().starts_with("bridge_dev_source") && l.contains(&config.attest_program.to_string())
            })
        {
            bail!("this config names the DEV statement source as attest program; it is for chain 31337 only");
        }
        Ok(Self {
            bridge_program,
            config,
            instance,
            vault_block: v["vault_block"].as_u64().unwrap_or(0),
            anchor: v["anchor"].as_str().map(parse_addr).transpose()?,
        })
    }
}

fn parse_token_spec(s: &str) -> Result<TokenEntry> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 2 && parts.len() != 4 {
        bail!("--token TOKEN:DENOM,DENOM[:MINT_FEE:BURN_FEE]");
    }
    let denoms = parts[1]
        .split(',')
        .map(str::parse::<u128>)
        .collect::<Result<Vec<_>, _>>()?;
    let (mint_fee, burn_fee) = if parts.len() == 4 {
        (parts[2].parse()?, parts[3].parse()?)
    } else {
        (0, 0)
    };
    Ok(TokenEntry {
        token: token_arg(parts[0])?,
        denoms,
        mint_fee,
        burn_fee,
    })
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Config {
            out,
            bridge_program,
            chain_id,
            vault,
            vault_block,
            anchor,
            attest_program,
            attest_instance,
            tokens,
            treasury,
        } => {
            let config = BridgeConfig {
                chain_id,
                vault: parse_addr(&vault)?,
                attest_program,
                attest_instance,
                tokens: tokens.iter().map(|t| parse_token_spec(t)).collect::<Result<_>>()?,
                treasury,
            };
            let instance = instance_id(&bridge_program, &config);
            let v = json!({
                "bridge_program": bridge_program.to_string(),
                "bridge_program_hex": hex0x(bridge_program.value()),
                "instance": instance.to_string(),
                "instance_hex": hex0x(instance.value()),
                "chain_id": chain_id,
                "vault": vault,
                "vault_block": vault_block,
                "anchor": anchor,
                "attest_program": attest_program.to_string(),
                "attest_instance": attest_instance.to_string(),
                "tokens": tokens,
                "treasury": treasury.map(|t| t.to_string()),
                "config_borsh": hex0x(&borsh::to_vec(&config)?),
            });
            std::fs::write(&out, serde_json::to_string_pretty(&v)? + "\n")?;
            println!("{}", serde_json::to_string_pretty(&v)?);
        }
        Cmd::Commitment {
            index,
            dest,
            token,
            denom,
        } => {
            let cf = ConfigFile::load(cli.config.as_ref())?;
            let key = keys::load_key()?;
            let token = token_arg(&token)?;
            let entry = cf.config.token(&token).context("token not in the bridge config")?;
            if !entry.denoms.contains(&denom) {
                bail!("{denom} is not a permitted denomination: {:?}", entry.denoms);
            }
            let secret = deposit_secret(&key, cf.config.chain_id, &cf.config.vault, index);
            let c = commitment(&secret, &dest_binding(cf.instance.value(), dest.value()), &token, denom);
            let denom_index = entry.denoms.iter().position(|d| *d == denom).unwrap_or_default();
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "commitment": hex0x(&c),
                    "leaf": hex0x(&leaf(&token, denom, &c)),
                    "token": hex0x(&token),
                    "denom": denom.to_string(),
                    "denom_index": denom_index,
                    "deposit_index": index,
                    "public_on_ethereum": "depositor address, token, amount, time, commitment (random-looking)",
                    "hidden": "the LEZ destination (bound inside the commitment)",
                }))?
            );
        }
        Cmd::AttestRoot {
            slot,
            min_eth_block,
            proofgen,
        } => {
            let cf = ConfigFile::load(cli.config.as_ref())?;
            lezops::attest_root(&cf, &Rpc::new(&cli.rpc), slot, min_eth_block, proofgen).await?;
        }
        Cmd::Claim {
            index,
            dest,
            root,
            claimer,
            public,
            force,
        } => {
            let cf = ConfigFile::load(cli.config.as_ref())?;
            let key = keys::load_key()?;
            let root = parse_b32(&root)?;
            let rpc = Rpc::new(&cli.rpc);
            let args = build_claim(&cf, &rpc, &key, index, dest, root).await?;
            lezops::claim(&cf, args, claimer.unwrap_or(dest), public, force).await?;
        }
        Cmd::Burn {
            index,
            holder,
            token,
            denom,
            recipient,
            fee,
            public,
        } => {
            let cf = ConfigFile::load(cli.config.as_ref())?;
            let key = keys::load_key()?;
            let token = token_arg(&token)?;
            let recipient = parse_addr(&recipient)?;
            if fee >= denom {
                bail!("fee must be below the denomination");
            }
            let secret = burn_secret(&key, cf.instance.value(), index);
            let note = burn_note(&secret, &recipient, fee);
            let memo = memo_seal(&secret, &recipient, fee);
            lezops::burn(&cf, holder, public, token, denom, note, memo).await?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "burn_index": index,
                    "note": hex0x(&note),
                    "leaf": hex0x(&burn_leaf(&token, denom, &note)),
                    "public_on_lez": "token, amount, time, note and memo (both random-looking)",
                    "hidden": "the Ethereum recipient and the relayer fee",
                }))?
            );
        }
        Cmd::AccumulatorRoot => {
            let cf = ConfigFile::load(cli.config.as_ref())?;
            let acc = lezops::read_accumulator(&cf).await?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "instance_hex": hex0x(cf.instance.value()),
                    "root": hex0x(&acc.root),
                    "count": acc.tree.count,
                }))?
            );
        }
        Cmd::Prove { index, out, mode } => {
            let cf = ConfigFile::load(cli.config.as_ref())?;
            let key = keys::load_key()?;
            let witness = lezops::burn_witness(&cf, &key, index).await?;
            prove::run(&witness, &out, &mode)?;
        }
        Cmd::Recover { dests, gap, lez } => {
            let cf = ConfigFile::load(cli.config.as_ref())?;
            let key = keys::load_key()?;
            recover(&cf, &Rpc::new(&cli.rpc), &key, &dests, gap, lez).await?;
        }
        Cmd::DevAttestRoot => {
            let cf = ConfigFile::load(cli.config.as_ref())?;
            lezops::dev_attest_root(&cf, &Rpc::new(&cli.rpc)).await?;
        }
        Cmd::InstanceHead => {
            let cf = ConfigFile::load(cli.config.as_ref())?;
            lezops::instance_head(&cf, &Rpc::new(&cli.rpc)).await?;
        }
        Cmd::ImageId => {
            let elf = std::fs::read(attest_host::guests_dir().join("burn_proof.bin"))?;
            println!("{}", hex0x(risc0_zkvm::compute_image_id(&elf)?.as_bytes()));
        }
        Cmd::Vectors { out } => prove::vectors(&out)?,
    }
    Ok(())
}

/// Finds deposit `index`'s leaf among the vault's logs, and its path to `root` (a root the vault really had: the
/// `root` field of some deposit log).
async fn build_claim(
    cf: &ConfigFile,
    rpc: &Rpc,
    key: &B32,
    index: u64,
    dest: AccountId,
    root: B32,
) -> Result<bridge_program::lez::ClaimArgs> {
    let secret = deposit_secret(key, cf.config.chain_id, &cf.config.vault, index);
    let binding = dest_binding(cf.instance.value(), dest.value());
    let head = rpc.block_number().await?;
    let logs = rpc.deposit_logs(&cf.config.vault, cf.vault_block, head).await?;
    let end = logs
        .iter()
        .find(|l| l.root == root)
        .context("no deposit log has this root")?;
    let leaves: Vec<B32> = logs
        .iter()
        .filter(|l| l.leaf_index <= end.leaf_index)
        .map(|l| leaf(&l.token, l.denom, &l.commitment))
        .collect();
    let mine = logs
        .iter()
        .filter(|l| l.leaf_index <= end.leaf_index)
        .find(|l| l.commitment == commitment(&secret, &binding, &l.token, l.denom))
        .context("deposit not found under this root for this index and destination")?;
    let (r, path) = path_for(&leaves, usize::try_from(mine.leaf_index)?);
    if r != root {
        bail!("rebuilt root differs from the log's root: logs incomplete?");
    }
    Ok(bridge_program::lez::ClaimArgs {
        config: cf.config.clone(),
        token: mine.token,
        denom: mine.denom,
        secret,
        dest,
        leaf_index: mine.leaf_index,
        path,
        root,
        attest: None,
    })
}

/// Recovery from the seed alone (F9, R5): deposits from vault logs, burns from the LEZ leaf records.
async fn recover(cf: &ConfigFile, rpc: &Rpc, key: &B32, dests: &[AccountId], gap: u64, lez: bool) -> Result<()> {
    let head = rpc.block_number().await?;
    let logs = rpc.deposit_logs(&cf.config.vault, cf.vault_block, head).await?;
    let mut deposits = Vec::new();
    let mut misses = 0;
    let mut i = 0_u64;
    while misses < gap {
        let secret = deposit_secret(key, cf.config.chain_id, &cf.config.vault, i);
        let mut found = false;
        for d in dests {
            let binding = dest_binding(cf.instance.value(), d.value());
            for l in &logs {
                if l.commitment == commitment(&secret, &binding, &l.token, l.denom) {
                    found = true;
                    let n = deposit_nullifier(&secret);
                    let claimed = if lez {
                        Some(lezops::is_claimed(cf, &n).await?)
                    } else {
                        None
                    };
                    deposits.push(json!({
                        "deposit_index": i, "dest": d.to_string(), "token": hex0x(&l.token),
                        "denom": l.denom.to_string(), "leaf_index": l.leaf_index, "eth_block": l.block,
                        "eth_tx": l.tx, "claimed": claimed,
                    }));
                }
            }
        }
        misses = if found { 0 } else { misses + 1 };
        i += 1;
    }
    let mut burns = Vec::new();
    if lez {
        let records = lezops::burn_records(cf).await?;
        let mut misses = 0;
        let mut j = 0_u64;
        while misses < gap {
            let secret = burn_secret(key, cf.instance.value(), j);
            let mut found = false;
            for r in &records {
                let (recipient, fee) = memo_open(&secret, &r.memo);
                if burn_note(&secret, &recipient, fee) == r.note {
                    found = true;
                    let released = rpc
                        .burn_nullifier_spent(&cf.config.vault, &burn_nullifier(&secret))
                        .await?;
                    burns.push(json!({
                        "burn_index": j, "leaf_index": r.index, "token": hex0x(&r.token),
                        "denom": r.denom.to_string(), "recipient": hex0x(&recipient), "fee": fee.to_string(),
                        "released": released,
                    }));
                }
            }
            misses = if found { 0 } else { misses + 1 };
            j += 1;
        }
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({"deposits": deposits, "burns": burns}))?
    );
    Ok(())
}
