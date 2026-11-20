//! Synthetic blocks built from the mirror types: the chain check and the burn finder accept the honest case and
//! refuse each listed tampering. Real-chain checks against LEZ's own types are in tools/lez_chain_probe.

use lez_chain::*;

const BRIDGE: AccountId = AccountId([7; 32]);
const INSTANCE: AccountId = AccountId([9; 32]);
const NOTE: Hash32 = [5; 32];

fn context() -> PublicExecutionContext {
    PublicExecutionContext {
        actors: vec![],
        authorized_accounts: vec![],
        promotions: vec![],
    }
}

fn append_bytes(instance: AccountId, note: Hash32) -> Vec<u8> {
    let mut m = vec![BRIDGE_TAG_APPEND];
    m.extend(borsh::to_vec(&(instance, [1_u8; 20], 10_u128.pow(16), note, [3_u8; 36])).unwrap());
    m
}

fn private_burn(from_program: AccountId, to: Actor, message: Vec<u8>) -> LeeTransaction {
    let delivery = Delivery {
        envelope: MessageEnvelope {
            from: Actor {
                account_id: AccountId([4; 32]),
                program_account_id: from_program,
            },
            to,
            message,
        },
        inherited_authorizations: vec![],
        inherits_entry_authorizations: false,
        pda_seeds: vec![],
    };
    LeeTransaction::PrivacyPreserving(PrivacyPreservingTransaction {
        message: TransactionMessage {
            public_context: context(),
            execution: ProvenExecution {
                boundary: vec![BoundaryStep::PrivateToPublic(delivery), BoundaryStep::EndPublicSubtree],
                async_messages: vec![],
                recovery_bindings: vec![],
                public_root: None,
                private_actions: vec![],
                validity: ValidityWindows {
                    blocks: (None, None),
                    timestamps: (None, None),
                },
                program_image_claims: vec![],
                fee: None,
            },
            nonces: vec![],
            admission_evidence: vec![],
        },
        witness_set: PrivacyWitnessSet {
            signatures_and_public_keys: vec![],
            proof: vec![0xaa; 64],
        },
    })
}

fn public_call(to: Actor, message: Vec<u8>) -> LeeTransaction {
    LeeTransaction::Public(PublicTransaction {
        message: TransactionMessage {
            public_context: context(),
            execution: PublicExecution {
                root: RootCall { to, message },
                fee: None,
            },
            nonces: vec![],
            admission_evidence: vec![],
        },
        witness_set: PublicWitnessSet {
            signatures_and_public_keys: vec![],
        },
    })
}

fn accumulator() -> Actor {
    Actor {
        account_id: accumulator_id(&BRIDGE, &INSTANCE),
        program_account_id: BRIDGE,
    }
}

/// Encodes a block with its hash recomputed the way LEZ does.
fn block(id: u64, prev: Hash32, transactions: Vec<LeeTransaction>, reverted: Vec<u64>) -> Vec<u8> {
    let mut b = Block {
        header: BlockHeader {
            block_id: id,
            prev_block_hash: prev,
            hash: [0; 32],
            timestamp: 1_000 + id,
        },
        body: BlockBody { transactions, reverted },
        bedrock_status: BedrockStatus::Pending,
    };
    let raw = borsh::to_vec(&b).unwrap();
    b.header.hash = block_hash(&raw).unwrap();
    borsh::to_vec(&b).unwrap()
}

fn chain(burn_block: Vec<LeeTransaction>, reverted: Vec<u64>) -> (Tip, Vec<Vec<u8>>) {
    let pinned = Tip {
        block_id: 10,
        hash: [0x10; 32],
    };
    let b11 = block(11, pinned.hash, vec![], vec![]);
    let b12 = block(12, block_hash(&b11).unwrap(), burn_block, reverted);
    let b13 = block(13, block_hash(&b12).unwrap(), vec![], vec![]);
    (pinned, vec![b11, b12, b13])
}

fn honest() -> Vec<LeeTransaction> {
    vec![
        public_call(
            Actor {
                account_id: AccountId([2; 32]),
                program_account_id: AccountId([3; 32]),
            },
            vec![1, 2],
        ),
        private_burn(BRIDGE, accumulator(), append_bytes(INSTANCE, NOTE)),
    ]
}

#[test]
fn honest_chain_and_burn_are_accepted() {
    let (pinned, blocks) = chain(honest(), vec![]);
    let tip = verify_chain(pinned, blocks.iter().map(Vec::as_slice)).unwrap();
    assert_eq!(tip.block_id, 13);
    let b = parse_block(&blocks[1]).unwrap();
    assert_eq!(borsh::to_vec(&b).unwrap(), blocks[1]);
    let burn = find_burn(&b, 1, &BRIDGE, &INSTANCE, &NOTE).unwrap();
    assert_eq!(burn.route, BurnRoute::PrivateToPublic { step: 0 });
    assert_eq!(burn.denom, 10_u128.pow(16));
}

#[test]
fn chain_refusals() {
    let (pinned, blocks) = chain(honest(), vec![]);
    let run = |bs: &[Vec<u8>]| verify_chain(pinned, bs.iter().map(Vec::as_slice));
    // Wrong pinned hash.
    assert_eq!(
        verify_chain(
            Tip {
                block_id: 10,
                hash: [0x11; 32]
            },
            blocks.iter().map(Vec::as_slice)
        ),
        Err(ChainError::BrokenLink { block_id: 11 })
    );
    // Id gap (block 12 left out) and a repeated block.
    assert_eq!(
        run(&[blocks[0].clone(), blocks[2].clone()]),
        Err(ChainError::IdGap { expected: 12, got: 13 })
    );
    assert_eq!(
        run(&[blocks[0].clone(), blocks[0].clone()]),
        Err(ChainError::IdGap { expected: 12, got: 11 })
    );
    // One body byte changed: the stored hash no longer matches; with the stored hash fixed, the next link breaks.
    let mut t = blocks.clone();
    let last = t[1].len() - 2;
    t[1][last] ^= 1;
    assert_eq!(run(&t), Err(ChainError::StoredHash { block_id: 12 }));
    let h = block_hash(&t[1]).unwrap();
    t[1][40..72].copy_from_slice(&h);
    assert_eq!(run(&t), Err(ChainError::BrokenLink { block_id: 13 }));
}

#[test]
fn burn_refusals() {
    // Listed as reverted.
    let (_, blocks) = chain(honest(), vec![1]);
    assert_eq!(
        find_burn(&parse_block(&blocks[1]).unwrap(), 1, &BRIDGE, &INSTANCE, &NOTE),
        Err(ChainError::Reverted)
    );
    // Append bytes inside another transaction's data (a public call to another program).
    let other = Actor {
        account_id: AccountId([2; 32]),
        program_account_id: AccountId([3; 32]),
    };
    let (_, blocks) = chain(vec![public_call(other, append_bytes(INSTANCE, NOTE))], vec![]);
    assert_eq!(
        find_burn(&parse_block(&blocks[1]).unwrap(), 0, &BRIDGE, &INSTANCE, &NOTE),
        Err(ChainError::NotABurn)
    );
    // Append not sent by the bridge program.
    let (_, blocks) = chain(
        vec![private_burn(
            AccountId([8; 32]),
            accumulator(),
            append_bytes(INSTANCE, NOTE),
        )],
        vec![],
    );
    assert_eq!(
        find_burn(&parse_block(&blocks[1]).unwrap(), 0, &BRIDGE, &INSTANCE, &NOTE),
        Err(ChainError::NotABurn)
    );
    // Another instance, another note, another accumulator.
    let (_, blocks) = chain(honest(), vec![]);
    let b = parse_block(&blocks[1]).unwrap();
    assert_eq!(
        find_burn(&b, 1, &BRIDGE, &AccountId([1; 32]), &NOTE),
        Err(ChainError::NotABurn)
    );
    assert_eq!(
        find_burn(&b, 1, &BRIDGE, &INSTANCE, &[6; 32]),
        Err(ChainError::NotABurn)
    );
    assert_eq!(
        find_burn(&b, 2, &BRIDGE, &INSTANCE, &NOTE),
        Err(ChainError::NoSuchTransaction)
    );
    let wrong_acc = Actor {
        account_id: AccountId([0xee; 32]),
        program_account_id: BRIDGE,
    };
    let (_, blocks) = chain(
        vec![private_burn(BRIDGE, wrong_acc, append_bytes(INSTANCE, NOTE))],
        vec![],
    );
    assert_eq!(
        find_burn(&parse_block(&blocks[1]).unwrap(), 0, &BRIDGE, &INSTANCE, &NOTE),
        Err(ChainError::NotABurn)
    );
    // Trailing bytes after a block do not parse.
    let mut t = blocks[1].clone();
    t.push(0);
    assert_eq!(parse_block(&t), Err(ChainError::Parse));
}
