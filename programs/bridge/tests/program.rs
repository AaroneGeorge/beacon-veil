//! Native tests of program `bridge` (`bridge_program::program::handle`): each handler, its accounts and its refusals.
//! The same function runs in the guest; on chain these panics reject the whole transaction.

use attest_core::{AccountId, AttestMessage, ClaimKind, HeaderRecord, Statement};
use bridge_program::{
    DEPOSIT_ROOT_SLOT, NATIVE,
    hash::{burn_leaf, commitment, deposit_nullifier, dest_binding, leaf},
    lez::{
        Accumulator, BridgeConfig, BridgeMessage, BurnEvent, ClaimArgs, Holding, TokenEntry, accumulator_seed,
        instance_id, leaf_record_seed, nullifier_seed, pda, root_seed,
    },
    program::handle,
    tree::{IncrementalTree, path_for},
};
use lee_core::{
    account::Actor,
    program::{ReceiveInput, Response},
};

const DENOM: u128 = 10_000_000_000_000_000;

fn bridge() -> AccountId {
    AccountId::new([0xB0; 32])
}
fn attest() -> AccountId {
    AccountId::new([0xA7; 32])
}
fn attest_instance() -> AccountId {
    AccountId::new([0xA1; 32])
}
fn claimer() -> AccountId {
    AccountId::new([0xC1; 32])
}

fn config(mint_fee: u128, treasury: Option<AccountId>) -> BridgeConfig {
    BridgeConfig {
        chain_id: 11_155_111,
        vault: [0x5a; 20],
        attest_program: attest(),
        attest_instance: attest_instance(),
        tokens: vec![TokenEntry {
            token: NATIVE,
            denoms: vec![DENOM / 10, DENOM],
            mint_fee,
            burn_fee: 0,
        }],
        treasury,
    }
}

fn input(receiver: Actor, from: Option<Actor>, authorized: bool, pre: Vec<u8>, msg: Vec<u8>) -> ReceiveInput {
    ReceiveInput {
        receiver,
        from,
        is_authorized: authorized,
        pre_state: pre.into(),
        message: msg,
    }
}

fn expect_panic(code: &str, f: impl FnOnce() -> Response + std::panic::UnwindSafe) {
    let err = std::panic::catch_unwind(f)
        .err()
        .unwrap_or_else(|| panic!("expected a panic with {code}"));
    let text = err
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| err.downcast_ref::<&str>().map(|s| (*s).to_string()))
        .unwrap_or_default();
    assert!(text.contains(code), "expected {code}, got {text:?}");
}

/// Five deposits; ours is leaf 2, bound to `dest`.
fn claim_args(cfg: &BridgeConfig, dest: AccountId) -> ClaimArgs {
    let inst = instance_id(&bridge(), cfg);
    let secret = [0x5e; 32];
    let c = commitment(&secret, &dest_binding(inst.value(), dest.value()), &NATIVE, DENOM);
    let mut leaves: Vec<[u8; 32]> = (0_u8..5).map(|i| leaf(&NATIVE, DENOM, &[i; 32])).collect();
    leaves[2] = leaf(&NATIVE, DENOM, &c);
    let (root, path) = path_for(&leaves, 2);
    ClaimArgs {
        config: cfg.clone(),
        token: NATIVE,
        denom: DENOM,
        secret,
        dest,
        leaf_index: 2,
        path,
        root,
        attest: None,
    }
}

fn claim_msg(a: &ClaimArgs) -> Vec<u8> {
    borsh::to_vec(&BridgeMessage::Claim(Box::new(a.clone()))).unwrap()
}

fn statement(cfg: &BridgeConfig, root: [u8; 32]) -> Statement {
    let mut subject = cfg.vault.to_vec();
    subject.extend_from_slice(&DEPOSIT_ROOT_SLOT);
    Statement {
        instance: cfg.attest_instance,
        chain_id: cfg.chain_id,
        slot: 100,
        exec_number: 7,
        kind: ClaimKind::Storage,
        subject,
        value: root.to_vec(),
        predicate_hash: [0; 32],
        id: cfg.root_statement_id(&root),
    }
}

fn receipt_of(cfg: &BridgeConfig, root: &[u8; 32]) -> (AccountId, lee_core::program::PdaSeed) {
    let seed = root_seed(&cfg.attest_program, &cfg.attest_instance, &cfg.root_statement_id(root));
    (pda(&bridge(), &seed), seed)
}

// ---------------------------------------------------------------- claim

#[test]
fn claim_to_own_private_account_credits_and_checks() {
    let cfg = config(0, None);
    let a = claim_args(&cfg, claimer());
    let inst = instance_id(&bridge(), &cfg);
    let r = handle(&input(
        Actor::new(claimer(), bridge()),
        None,
        true,
        vec![],
        claim_msg(&a),
    ));
    let h = Holding::decode(r.post_state.as_ref().unwrap()).unwrap();
    assert_eq!(h.balance(&inst, &NATIVE), DENOM);
    assert_eq!(r.sync_messages.len(), 2);
    let (receipt, rseed) = receipt_of(&cfg, &a.root);
    assert_eq!(r.sync_messages[0].to, Actor::new(receipt, bridge()));
    assert!(r.sync_messages[0].pda_seeds.contains(&rseed));
    let nseed = nullifier_seed(&inst, &deposit_nullifier(&a.secret));
    assert_eq!(r.sync_messages[1].to, Actor::new(pda(&bridge(), &nseed), bridge()));
    assert!(r.events.is_empty(), "a claim emits no event");
}

#[test]
fn claim_to_public_dest_sends_credit_and_fee() {
    let treasury = AccountId::new([0x7e; 32]);
    let cfg = config(DENOM / 100, Some(treasury));
    let dest = AccountId::new([0xD0; 32]);
    let a = claim_args(&cfg, dest);
    let r = handle(&input(
        Actor::new(claimer(), bridge()),
        None,
        true,
        vec![],
        claim_msg(&a),
    ));
    assert!(r.post_state.is_none());
    assert_eq!(r.sync_messages.len(), 4);
    let inst = instance_id(&bridge(), &cfg);
    assert_eq!(r.sync_messages[2].to, Actor::new(dest, bridge()));
    assert_eq!(
        r.sync_messages[2].message,
        borsh::to_vec(&BridgeMessage::Credit {
            instance: inst,
            token: NATIVE,
            amount: DENOM - DENOM / 100
        })
        .unwrap()
    );
    assert_eq!(r.sync_messages[3].to, Actor::new(treasury, bridge()));
}

#[test]
fn claim_with_attest_leg_calls_attest_first() {
    let cfg = config(0, None);
    let mut a = claim_args(&cfg, claimer());
    a.attest = Some(bridge_program::lez::AttestCall {
        on: claimer(),
        record: HeaderRecord::default(),
        claim: vec![1],
    });
    let r = handle(&input(
        Actor::new(claimer(), bridge()),
        None,
        true,
        vec![],
        claim_msg(&a),
    ));
    assert_eq!(r.sync_messages.len(), 3);
    assert_eq!(r.sync_messages[0].to, Actor::new(claimer(), attest()));
    let m: AttestMessage = borsh::from_slice(&r.sync_messages[0].message).unwrap();
    let AttestMessage::Attest {
        instance, deliver_to, ..
    } = m
    else {
        panic!("not Attest")
    };
    assert_eq!(instance, attest_instance());
    assert_eq!(deliver_to, Actor::new(receipt_of(&cfg, &a.root).0, bridge()));
}

#[test]
fn claim_refuses_wrong_path_wrong_dest_wrong_token_wrong_denom() {
    let cfg = config(0, None);
    let good = claim_args(&cfg, claimer());
    let mut bad_path = good.clone();
    bad_path.path[0][0] ^= 1;
    expect_panic("ESA:E07", move || {
        handle(&input(
            Actor::new(claimer(), bridge()),
            None,
            true,
            vec![],
            claim_msg(&bad_path),
        ))
    });
    // Commitment bound to `claimer`; a relayer redirecting to another account fails (Security #3).
    let mut redirected = good.clone();
    redirected.dest = AccountId::new([0xEE; 32]);
    expect_panic("ESA:E07", move || {
        handle(&input(
            Actor::new(claimer(), bridge()),
            None,
            true,
            vec![],
            claim_msg(&redirected),
        ))
    });
    let mut other_denom = good.clone();
    other_denom.denom = DENOM / 10;
    expect_panic("ESA:E07", move || {
        handle(&input(
            Actor::new(claimer(), bridge()),
            None,
            true,
            vec![],
            claim_msg(&other_denom),
        ))
    });
    let mut not_permitted = good.clone();
    not_permitted.denom = 12_345;
    expect_panic("WB:B02", move || {
        handle(&input(
            Actor::new(claimer(), bridge()),
            None,
            true,
            vec![],
            claim_msg(&not_permitted),
        ))
    });
    let mut token = good;
    token.token = [9; 20];
    expect_panic("WB:B01", move || {
        handle(&input(
            Actor::new(claimer(), bridge()),
            None,
            true,
            vec![],
            claim_msg(&token),
        ))
    });
}

#[test]
fn another_instance_uses_other_nullifier_and_receipt_namespace() {
    let a = config(0, None);
    let mut b = config(0, None);
    b.treasury = Some(AccountId::new([1; 32])); // any config change is another instance
    assert_ne!(instance_id(&bridge(), &a), instance_id(&bridge(), &b));
    let n = deposit_nullifier(&[0x5e; 32]);
    assert_ne!(
        pda(&bridge(), &nullifier_seed(&instance_id(&bridge(), &a), &n)),
        pda(&bridge(), &nullifier_seed(&instance_id(&bridge(), &b), &n))
    );
}

// ---------------------------------------------------------------- root receipt

#[test]
fn statement_lands_only_on_its_own_receipt() {
    let cfg = config(0, None);
    let root = [0x77; 32];
    let st = statement(&cfg, root);
    let (receipt, _) = receipt_of(&cfg, &root);
    let from = Some(Actor::new(AccountId::new([0x99; 32]), attest()));
    let r = handle(&input(
        Actor::new(receipt, bridge()),
        from,
        false,
        vec![],
        borsh::to_vec(&st).unwrap(),
    ));
    assert_eq!(r.post_state.unwrap().to_vec(), borsh::to_vec(&st).unwrap());
    // second delivery: keep
    let r2 = handle(&input(
        Actor::new(receipt, bridge()),
        from,
        false,
        borsh::to_vec(&st).unwrap(),
        borsh::to_vec(&st).unwrap(),
    ));
    assert!(r2.post_state.is_none());
    // a fake program cannot write the real receipt
    let fake = Some(Actor::new(AccountId::new([0x99; 32]), AccountId::new([0xFA; 32])));
    let st2 = st.clone();
    expect_panic("WB:B06", move || {
        handle(&input(
            Actor::new(receipt, bridge()),
            fake,
            false,
            vec![],
            borsh::to_vec(&st2).unwrap(),
        ))
    });
    // a fake light-client instance cannot either
    let mut st3 = st;
    st3.instance = AccountId::new([0xFB; 32]);
    expect_panic("WB:B06", move || {
        handle(&input(
            Actor::new(receipt, bridge()),
            from,
            false,
            vec![],
            borsh::to_vec(&st3).unwrap(),
        ))
    });
}

#[test]
fn check_root_needs_the_statement() {
    let cfg = config(0, None);
    let root = [0x77; 32];
    let (receipt, _) = receipt_of(&cfg, &root);
    let me = Some(Actor::new(claimer(), bridge()));
    let msg = borsh::to_vec(&BridgeMessage::CheckRoot {
        attest_program: cfg.attest_program,
        attest_instance: cfg.attest_instance,
        chain_id: cfg.chain_id,
        vault: cfg.vault,
        root,
    })
    .unwrap();
    let ok = handle(&input(
        Actor::new(receipt, bridge()),
        me,
        false,
        borsh::to_vec(&statement(&cfg, root)).unwrap(),
        msg.clone(),
    ));
    assert!(ok.post_state.is_none());
    let m = msg.clone();
    expect_panic("WB:B03", move || {
        handle(&input(Actor::new(receipt, bridge()), me, false, vec![], m))
    });
    let m = msg.clone();
    let wrong = borsh::to_vec(&statement(&cfg, [0x78; 32])).unwrap();
    expect_panic("WB:B03", move || {
        handle(&input(Actor::new(receipt, bridge()), me, false, wrong, m))
    });
    let m = msg;
    let st = borsh::to_vec(&statement(&cfg, root)).unwrap();
    expect_panic("WB:B05", move || {
        handle(&input(Actor::new(receipt, bridge()), None, true, st, m))
    });
}

// ---------------------------------------------------------------- nullifier, credit

#[test]
fn nullifier_marks_once() {
    let inst = AccountId::new([3; 32]);
    let n = [4; 32];
    let acct = pda(&bridge(), &nullifier_seed(&inst, &n));
    let me = Some(Actor::new(claimer(), bridge()));
    let msg = borsh::to_vec(&BridgeMessage::Nullify {
        instance: inst,
        nullifier: n,
    })
    .unwrap();
    let r = handle(&input(Actor::new(acct, bridge()), me, false, vec![], msg.clone()));
    assert_eq!(r.post_state.unwrap().to_vec(), vec![1]);
    let m = msg.clone();
    expect_panic("ESA:E11", move || {
        handle(&input(Actor::new(acct, bridge()), me, false, vec![1], m))
    });
    let m = msg.clone();
    expect_panic("WB:B06", move || {
        handle(&input(Actor::new(claimer(), bridge()), me, false, vec![], m))
    });
    let m = msg;
    expect_panic("WB:B05", move || {
        handle(&input(Actor::new(acct, bridge()), None, true, vec![], m))
    });
}

#[test]
fn credit_only_from_own_program() {
    let inst = AccountId::new([3; 32]);
    let msg = borsh::to_vec(&BridgeMessage::Credit {
        instance: inst,
        token: NATIVE,
        amount: 5,
    })
    .unwrap();
    let me = Some(Actor::new(claimer(), bridge()));
    let r = handle(&input(
        Actor::new(AccountId::new([8; 32]), bridge()),
        me,
        false,
        vec![],
        msg.clone(),
    ));
    assert_eq!(
        Holding::decode(r.post_state.as_ref().unwrap())
            .unwrap()
            .balance(&inst, &NATIVE),
        5
    );
    let m = msg.clone();
    expect_panic("WB:B05", move || {
        handle(&input(
            Actor::new(AccountId::new([8; 32]), bridge()),
            None,
            true,
            vec![],
            m,
        ))
    });
    // a message from another program is read as a statement delivery, never as a credit
    let other = Some(Actor::new(claimer(), AccountId::new([0xFA; 32])));
    expect_panic("ESA:E01", move || {
        handle(&input(
            Actor::new(AccountId::new([8; 32]), bridge()),
            other,
            true,
            vec![],
            msg,
        ))
    });
}

// ---------------------------------------------------------------- burn, append

fn holding_with(cfg: &BridgeConfig, amount: u128) -> Vec<u8> {
    let mut h = Holding::default();
    h.set(instance_id(&bridge(), cfg), NATIVE, amount);
    borsh::to_vec(&h).unwrap()
}

#[test]
fn burn_debits_and_appends() {
    let cfg = config(0, None);
    let inst = instance_id(&bridge(), &cfg);
    let msg = borsh::to_vec(&BridgeMessage::Burn {
        config: cfg.clone(),
        token: NATIVE,
        denom: DENOM,
        note: [6; 32],
        memo: [7; 36],
    })
    .unwrap();
    let r = handle(&input(
        Actor::new(claimer(), bridge()),
        None,
        true,
        holding_with(&cfg, DENOM + 3),
        msg.clone(),
    ));
    assert_eq!(
        Holding::decode(r.post_state.as_ref().unwrap())
            .unwrap()
            .balance(&inst, &NATIVE),
        3
    );
    let aseed = accumulator_seed(&inst);
    assert_eq!(r.sync_messages[0].to, Actor::new(pda(&bridge(), &aseed), bridge()));
    assert!(r.sync_messages[0].pda_seeds.contains(&aseed));
    let m = msg.clone();
    let c = cfg.clone();
    expect_panic("WB:B04", move || {
        handle(&input(
            Actor::new(claimer(), bridge()),
            None,
            true,
            holding_with(&c, DENOM - 1),
            m,
        ))
    });
    let m = msg;
    expect_panic("WB:B05", move || {
        handle(&input(
            Actor::new(claimer(), bridge()),
            None,
            false,
            holding_with(&cfg, DENOM),
            m,
        ))
    });
}

#[test]
fn append_matches_tree_and_records_leaf() {
    let inst = AccountId::new([3; 32]);
    let acc_id = pda(&bridge(), &accumulator_seed(&inst));
    let me = Some(Actor::new(claimer(), bridge()));
    let mut pre = vec![];
    let mut tree = IncrementalTree::default();
    for i in 0..3_u8 {
        let msg = borsh::to_vec(&BridgeMessage::Append {
            instance: inst,
            token: NATIVE,
            denom: DENOM,
            note: [i; 32],
            memo: [i; 36],
        })
        .unwrap();
        let r = handle(&input(Actor::new(acc_id, bridge()), me, false, pre, msg));
        tree.insert(burn_leaf(&NATIVE, DENOM, &[i; 32]));
        let acc: Accumulator = borsh::from_slice(r.post_state.as_ref().unwrap()).unwrap();
        assert_eq!(acc.root, tree.root());
        assert_eq!(acc.tree.count, u64::from(i) + 1);
        assert_eq!(r.events.len(), 1);
        let lseed = leaf_record_seed(&inst, u64::from(i));
        assert_eq!(r.sync_messages[0].to, Actor::new(pda(&bridge(), &lseed), bridge()));
        let BridgeMessage::RecordLeaf(ev) = borsh::from_slice(&r.sync_messages[0].message).unwrap() else {
            panic!()
        };
        assert_eq!(ev.index, u64::from(i));
        pre = r.post_state.unwrap().to_vec();
    }
    let wrong = Actor::new(claimer(), bridge());
    let msg = borsh::to_vec(&BridgeMessage::Append {
        instance: inst,
        token: NATIVE,
        denom: DENOM,
        note: [0; 32],
        memo: [0; 36],
    })
    .unwrap();
    expect_panic("WB:B06", move || handle(&input(wrong, me, false, vec![], msg)));
}

#[test]
fn leaf_record_writes_once() {
    let inst = AccountId::new([3; 32]);
    let ev = BurnEvent {
        instance: inst,
        index: 4,
        token: NATIVE,
        denom: DENOM,
        note: [1; 32],
        memo: [2; 36],
        root: [3; 32],
    };
    let at = Actor::new(pda(&bridge(), &leaf_record_seed(&inst, 4)), bridge());
    let me = Some(Actor::new(claimer(), bridge()));
    let msg = borsh::to_vec(&BridgeMessage::RecordLeaf(ev.clone())).unwrap();
    let r = handle(&input(at, me, false, vec![], msg.clone()));
    assert_eq!(r.post_state.unwrap().to_vec(), borsh::to_vec(&ev).unwrap());
    expect_panic("WB:B06", move || handle(&input(at, me, false, vec![1], msg)));
}
