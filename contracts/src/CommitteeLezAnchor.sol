// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

import {EIP712} from "@openzeppelin/contracts/utils/cryptography/EIP712.sol";
import {ECDSA} from "@openzeppelin/contracts/utils/cryptography/ECDSA.sol";
import {ILezAnchor} from "./ILezAnchor.sol";
import {ILezTipAnchor} from "./ILezTipAnchor.sol";

/// @title CommitteeLezAnchor (SPIKE: k-of-n signed tip, stand-in key set)
/// @notice THIS IS A SPIKE of the milestone-1 quorum check, run early: it measures the Ethereum side of a k-of-n
/// signature over a LEZ tip, not a quorum of LEZ's sequencers. LEZ v0.4.0-rc3 has no signature that a quorum puts on a
/// block (a block carries no signature; one sequencer signs the transaction that inscribes it; a threshold of the
/// accredited keys signs only channel configuration changes and transfers), so whoever signs here takes on a new
/// duty. In the sprint the signers are a stand-in set: secp256k1 keys made for the test, all held by one party, each
/// signer run reading one sequencer. LEZ's own sequencer keys are Ed25519. This does not meet RFP-022's rule of no
/// trusted signers (docs/bridge/TRUST.md).
///
/// What a signature says: "block `blockId` of zone `zone` has hash `blockHash`" (EIP-712 type `Tip`), or "`root` is
/// the burn-accumulator root of bridge instance `lezInstance` of zone `zone`" (type `Root`), for this anchor on this
/// chain. At least `threshold` distinct signers of the fixed set must sign; anyone may submit the signatures. A
/// statement is accepted `delay` seconds after it is attested (the delay stands in for a challenge window; nothing
/// acts on it). Two different hashes attested for one block id void that height for good: neither is final.
///
/// No owner, no setter, no upgrade: the signer set, the threshold, the delay and the zone are fixed at construction.
/// A new set is a new anchor, and so a new vault (the vault holds its anchors as immutables, Security #8).
contract CommitteeLezAnchor is ILezTipAnchor, ILezAnchor, EIP712 {
    uint256 public constant MIN_SIGNERS = 3;
    uint256 public constant MAX_SIGNERS = 16;
    bytes32 public constant TIP_TYPEHASH = keccak256("Tip(bytes32 zone,uint64 blockId,bytes32 blockHash)");
    bytes32 public constant ROOT_TYPEHASH = keccak256("Root(bytes32 zone,bytes32 lezInstance,bytes32 accumulatorRoot)");

    /// @notice Signatures needed per statement.
    uint8 public immutable threshold;
    /// @notice Seconds between attesting a statement and its acceptance. Stands in for the challenge window.
    uint64 public immutable delay;
    /// @notice Opaque id of the LEZ chain and zone the signers read.
    bytes32 public immutable zone;

    address[] private _signers;
    mapping(address => bool) public isSigner;

    /// @notice blockId => blockHash => time attested (0 = never).
    mapping(uint64 => mapping(bytes32 => uint64)) public tipAttestedAt;
    /// @notice blockId => the first hash attested for it.
    mapping(uint64 => bytes32) public tipHashOf;
    /// @notice blockId => two different hashes were attested for it; no hash at this height is final.
    mapping(uint64 => bool) public tipVoid;
    /// @notice lezInstance => accumulatorRoot => time attested (0 = never).
    mapping(bytes32 => mapping(bytes32 => uint64)) public rootAttestedAt;

    event TipAttested(uint64 indexed blockId, bytes32 indexed blockHash, uint64 finalAt, uint256 signerCount);
    event TipConflict(uint64 indexed blockId, bytes32 firstHash, bytes32 secondHash);
    event RootAttested(
        bytes32 indexed lezInstance, bytes32 indexed accumulatorRoot, uint64 finalAt, uint256 signerCount
    );

    error BadSignerSet();
    error TooFewSignatures();
    error NotASigner(address recovered);
    error SignersNotAscending();
    error AlreadyAttested();

    /// @param signers_ The fixed signer set, strictly ascending (which also refuses a repeated key), no zero address,
    /// `MIN_SIGNERS` to `MAX_SIGNERS` keys.
    /// @param threshold_ At least `lezThreshold(n)`, LEZ's own rule for channel configuration changes, and at most n.
    constructor(address[] memory signers_, uint8 threshold_, uint64 delay_, bytes32 zone_)
        EIP712("beacon-veil LEZ anchor", "1")
    {
        uint256 n = signers_.length;
        if (n < MIN_SIGNERS || n > MAX_SIGNERS) revert BadSignerSet();
        if (threshold_ < lezThreshold(n) || threshold_ > n) revert BadSignerSet();
        address last = address(0);
        for (uint256 i = 0; i < n; i++) {
            address s = signers_[i];
            if (s <= last) revert BadSignerSet(); // zero address, unsorted or repeated
            isSigner[s] = true;
            last = s;
        }
        _signers = signers_;
        threshold = threshold_;
        delay = delay_;
        zone = zone_;
    }

    /// @notice LEZ's threshold for a committee of `n` accredited keys: two thirds, rounded up, capped at n - 1,
    /// floored at 1 (`channel_config_threshold`, lez/sequencer/core/src/committee_discovery.rs, v0.4.0-rc3).
    function lezThreshold(uint256 n) public pure returns (uint256) {
        if (n <= 1) return 1;
        uint256 t = (2 * n + 2) / 3;
        return t > n - 1 ? n - 1 : t;
    }

    /// @notice The signer set, ascending.
    function signers() external view returns (address[] memory) {
        return _signers;
    }

    /// @notice The EIP-712 digest a signer signs for a tip.
    function tipDigest(uint64 blockId, bytes32 blockHash) public view returns (bytes32) {
        return _hashTypedDataV4(keccak256(abi.encode(TIP_TYPEHASH, zone, blockId, blockHash)));
    }

    /// @notice The EIP-712 digest a signer signs for a burn-accumulator root.
    function rootDigest(bytes32 lezInstance, bytes32 accumulatorRoot) public view returns (bytes32) {
        return _hashTypedDataV4(keccak256(abi.encode(ROOT_TYPEHASH, zone, lezInstance, accumulatorRoot)));
    }

    /// @notice Records block (`blockId`, `blockHash`) as attested. Anyone may call. `sigs` are 65-byte signatures over
    /// `tipDigest(blockId, blockHash)`, at least `threshold`, ordered by signer address, strictly ascending.
    function attestTip(uint64 blockId, bytes32 blockHash, bytes[] calldata sigs) external {
        if (tipAttestedAt[blockId][blockHash] != 0) revert AlreadyAttested();
        uint256 count = _checkQuorum(tipDigest(blockId, blockHash), sigs);
        uint64 nowTs = uint64(block.timestamp);
        tipAttestedAt[blockId][blockHash] = nowTs;
        bytes32 first = tipHashOf[blockId];
        if (first == bytes32(0)) {
            tipHashOf[blockId] = blockHash;
        } else {
            // A threshold signed two hashes at one height: evidence of a faulty set. No hash here is final.
            tipVoid[blockId] = true;
            emit TipConflict(blockId, first, blockHash);
        }
        emit TipAttested(blockId, blockHash, nowTs + delay, count);
    }

    /// @notice Records `accumulatorRoot` of `lezInstance` as attested. Anyone may call. Signatures as for `attestTip`,
    /// over `rootDigest(lezInstance, accumulatorRoot)`.
    function attestRoot(bytes32 lezInstance, bytes32 accumulatorRoot, bytes[] calldata sigs) external {
        if (rootAttestedAt[lezInstance][accumulatorRoot] != 0) revert AlreadyAttested();
        uint256 count = _checkQuorum(rootDigest(lezInstance, accumulatorRoot), sigs);
        uint64 nowTs = uint64(block.timestamp);
        rootAttestedAt[lezInstance][accumulatorRoot] = nowTs;
        emit RootAttested(lezInstance, accumulatorRoot, nowTs + delay, count);
    }

    /// @inheritdoc ILezTipAnchor
    function isFinalTip(uint64 blockId, bytes32 blockHash) external view returns (bool) {
        uint64 t = tipAttestedAt[blockId][blockHash];
        return t != 0 && !tipVoid[blockId] && block.timestamp >= uint256(t) + delay;
    }

    /// @inheritdoc ILezAnchor
    function isFinal(bytes32 lezInstance, bytes32 accumulatorRoot) external view returns (bool) {
        uint64 t = rootAttestedAt[lezInstance][accumulatorRoot];
        return t != 0 && block.timestamp >= uint256(t) + delay;
    }

    /// Every signature must recover to a member of the set, in strictly ascending order, so no signer counts twice.
    /// `ECDSA.recover` refuses malformed signatures and high-s signatures.
    function _checkQuorum(bytes32 digest, bytes[] calldata sigs) private view returns (uint256) {
        if (sigs.length < threshold) revert TooFewSignatures();
        address last = address(0);
        for (uint256 i = 0; i < sigs.length; i++) {
            address r = ECDSA.recover(digest, sigs[i]);
            if (!isSigner[r]) revert NotASigner(r);
            if (r <= last) revert SignersNotAscending();
            last = r;
        }
        return sigs.length;
    }
}
