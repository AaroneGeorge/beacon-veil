//! Refusals on a real burn block. Runs when `LEZ_CHAIN_SEQ_DEV_BLOCKS` names a directory of seq-dev blocks
//! (`<id>.borsh`, from `tools/lez_chain_probe/dbdump`); skipped otherwise. Burn 0 of 2026-11-12: block 2586, tx 0
//! (`evidence/bridge/groth16-release-v3/raw/burn-0.json`), bridge `7ahb3h5n...`, instance `BTpCBokr...`.

use lez_chain::*;

fn h(s: &str) -> [u8; 32] {
    let v: Vec<u8> = (0..64)
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect();
    v.try_into().unwrap()
}

#[test]
fn real_burn_block_refusals() {
    let Ok(dir) = std::env::var("LEZ_CHAIN_SEQ_DEV_BLOCKS") else {
        eprintln!("LEZ_CHAIN_SEQ_DEV_BLOCKS unset: skipped");
        return;
    };
    let read = |i: u64| std::fs::read(format!("{dir}/{i}.borsh")).unwrap();
    let bridge = AccountId(h("61c82e1d2abcd2a735763cfc988164881a5a22ebc0022e6af275ae99347be64e"));
    let instance = AccountId(h("9b7354d99185285a50471440e4f21809ffe9fa18bbc0a605007b7b891b399349"));
    let note = h("a78d5c17f272b7644439d956aed272ab8058acc0c354a7f9cb7c143c6ac78d85");

    let raw = read(2586);
    let block = parse_block(&raw).unwrap();
    let burn = find_burn(&block, 0, &bridge, &instance, &note).unwrap();
    assert_eq!((burn.denom, burn.token), (1_000_000_000_000_000, [0; 20]));

    // Reverted: the same block with tx 0 listed (its hash then changes, so the chain check refuses it too).
    let mut rev = block.clone();
    rev.body.reverted.push(0);
    assert_eq!(find_burn(&rev, 0, &bridge, &instance, &note), Err(ChainError::Reverted));
    let rev_raw = borsh::to_vec(&rev).unwrap();
    let pinned = Tip {
        block_id: 2585,
        hash: block_hash(&read(2585)).unwrap(),
    };
    assert_eq!(
        verify_chain(pinned, [rev_raw.as_slice()]),
        Err(ChainError::StoredHash { block_id: 2586 })
    );

    // Another note, another instance, another bridge program, a tx that is a claim (block 2583: CheckRoot, Nullify).
    assert_eq!(
        find_burn(&block, 0, &bridge, &instance, &[1; 32]),
        Err(ChainError::NotABurn)
    );
    assert_eq!(
        find_burn(&block, 0, &bridge, &AccountId([1; 32]), &note),
        Err(ChainError::NotABurn)
    );
    assert_eq!(
        find_burn(&block, 0, &AccountId([1; 32]), &instance, &note),
        Err(ChainError::NotABurn)
    );
    assert_eq!(
        find_burn(&parse_block(&read(2583)).unwrap(), 0, &bridge, &instance, &note),
        Err(ChainError::NotABurn)
    );
    // Burn 1 is in block 2588, not 2586.
    let note1 = h("34a49463d904527cf9ddf685ee6e94efa97b8596dbeffef8f83f3b0239aac090");
    assert_eq!(
        find_burn(&block, 0, &bridge, &instance, &note1),
        Err(ChainError::NotABurn)
    );
    assert!(find_burn(&parse_block(&read(2588)).unwrap(), 0, &bridge, &instance, &note1).is_ok());

    // Chain: 2581 -> 2591 holds; leaving out 2584 is a gap; a block from another chain (seq-dev 2586 after a fake
    // previous block) breaks the link.
    let chain: Vec<Vec<u8>> = (2582..=2591).map(read).collect();
    let p = Tip {
        block_id: 2581,
        hash: block_hash(&read(2581)).unwrap(),
    };
    assert_eq!(verify_chain(p, chain.iter().map(Vec::as_slice)).unwrap().block_id, 2591);
    let gap: Vec<&[u8]> = chain
        .iter()
        .filter(|b| header(b).unwrap().block_id != 2584)
        .map(Vec::as_slice)
        .collect();
    assert_eq!(
        verify_chain(p, gap),
        Err(ChainError::IdGap {
            expected: 2584,
            got: 2585
        })
    );
    assert_eq!(
        verify_chain(
            Tip {
                block_id: 2585,
                hash: [0; 32]
            },
            [raw.as_slice()]
        ),
        Err(ChainError::BrokenLink { block_id: 2586 })
    );
}
