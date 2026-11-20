// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import {SafeERC20} from "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";
import {IRiscZeroVerifier} from "risc0/IRiscZeroVerifier.sol";
import {ILezAnchor} from "./ILezAnchor.sol";
import {ILezTipAnchor} from "./ILezTipAnchor.sol";
import {Vault} from "./Vault.sol";

/// @title VaultChain
/// @notice `Vault` plus a third release route, `releaseChain`, that takes no accumulator root from anyone. Guest
/// `burn_chain` (crates/lez_chain/src/statement.rs) proves that a burn transaction for this note is in LEZ block N,
/// that the block does not list it as reverted, and that blocks P..N..T are one unbroken hash chain from the pinned
/// block P (`pinnedId`, `pinnedHash`, immutables here). The tip anchor says the hash of block T is final.
///
/// Proven: the burn transaction for this note is transaction i of the block with hash H_N; the block does not list it
/// as reverted; its bytes show the bridge program delivering Append{instance, token, denom, note} to the instance's
/// accumulator (private burn), or a holder's Burn root call to the bridge with a config whose instance is this one
/// (public burn); and blocks P..N..T are one unbroken hash chain with rising ids from the pinned (P, hash).
/// Attested: that the hash of block T is on the real, final LEZ chain (`tipAnchor`; in the sprint `StubLezTipAnchor`,
/// one key after a delay).
/// Not proven, still trusted: that LEZ executed those blocks correctly (rc3 headers carry no state root and public
/// execution is unproven, so "not reverted" and "the bridge program sent it" rest on the sequencer having run and
/// verified the transaction), finality, and which chain is the real one (anyone can build a hash chain from the pinned
/// block; the pinned block names the chain, it does not stop a dishonest attester).
/// The public-burn route is tested only on a synthetic block: no public burn exists on any chain we run.
///
/// The routes share the nullifier map, so one burn pays once across `release`, `releaseOpen` and `releaseChain`.
/// `Vault` is unchanged, and its token state is private, so this route reads the token config through
/// `tokenConfig` and keeps its own per-token release window against the same `releaseCap`: in a vault that also
/// releases through the other routes, up to twice the cap can leave in one window. Caps are a stub (F11).
contract VaultChain is Vault {
    using SafeERC20 for IERC20;

    uint256 public constant CHAIN_JOURNAL_VERSION = 2;
    /// @notice Chain journal = 15 ABI words, see `ChainJournal`.
    uint256 public constant CHAIN_JOURNAL_LENGTH = 15 * 32;

    /// @notice Public output of guest `burn_chain`: `Vault.BurnJournal` with the accumulator root replaced by
    /// `pinnedId, pinnedHash, tipId, tipHash, burnBlockId`.
    struct ChainJournal {
        uint256 version;
        uint256 chainId;
        address vault;
        bytes32 lezProgram;
        bytes32 lezInstance;
        uint256 pinnedId;
        bytes32 pinnedHash;
        uint256 tipId;
        bytes32 tipHash;
        uint256 burnBlockId;
        bytes32 nullifier;
        address recipient;
        address token;
        uint256 denom;
        uint256 fee;
    }

    /// @notice The chain route's own settings, immutables of the vault.
    struct ChainRoute {
        ILezTipAnchor tipAnchor;
        bytes32 chainImageId;
        uint64 pinnedId;
        bytes32 pinnedHash;
    }

    ILezTipAnchor public immutable tipAnchor;
    bytes32 public immutable chainImageId;
    /// @notice The pinned LEZ block: the chain the proofs must start from.
    uint64 public immutable pinnedId;
    bytes32 public immutable pinnedHash;

    bool public chainRouteFrozen;
    mapping(address => uint256) public chainWindowStart;
    mapping(address => uint256) public chainReleasedInWindow;

    event ChainRouteFreezeChanged(bool frozen);
    event ChainRelease(uint256 indexed burnBlockId, uint256 tipId, bytes32 tipHash);

    error WrongPinnedBlock();
    error TipNotFinal();

    constructor(
        IRiscZeroVerifier verifier_,
        ILezAnchor anchor_,
        bytes32 burnImageId_,
        bytes32 lezZone_,
        bytes32 lezProgram_,
        address admin,
        uint256 capWindow_,
        ChainRoute memory route
    ) Vault(verifier_, anchor_, burnImageId_, lezZone_, lezProgram_, admin, capWindow_) {
        tipAnchor = route.tipAnchor;
        chainImageId = route.chainImageId;
        pinnedId = route.pinnedId;
        pinnedHash = route.pinnedHash;
    }

    function setChainRouteFreeze(bool frozen) external onlyRole(FREEZER_ROLE) {
        chainRouteFrozen = frozen;
        emit ChainRouteFreezeChanged(frozen);
    }

    /// @notice Pays out one burn proven from LEZ block bytes. `journal` is the 480-byte public output of guest
    /// `burn_chain`; `seal` its proof. The fee goes to the caller (a permissionless relayer); the recipient is bound in
    /// the proof.
    function releaseChain(bytes calldata seal, bytes calldata journal) external nonReentrant {
        if (chainRouteFrozen || zkRouteFrozen) revert RouteFrozen();
        if (journal.length != CHAIN_JOURNAL_LENGTH) revert BadJournal();
        ChainJournal memory j = abi.decode(journal, (ChainJournal));
        if (j.version != CHAIN_JOURNAL_VERSION) revert BadJournal();
        if (lezInstance == bytes32(0)) revert InstanceNotBound();
        if (
            j.chainId != block.chainid || j.vault != address(this) || j.lezProgram != lezProgram
                || j.lezInstance != lezInstance
        ) revert WrongPairing();
        if (j.pinnedId != pinnedId || j.pinnedHash != pinnedHash) revert WrongPinnedBlock();
        if (j.tipId > type(uint64).max || !tipAnchor.isFinalTip(uint64(j.tipId), j.tipHash)) revert TipNotFinal();
        _checkChainRelease(j.token, j.denom, j.fee, j.nullifier);

        // Reverts on an invalid seal, or when the emergency stop has stopped the verifier.
        verifier.verify(seal, chainImageId, sha256(journal));

        _settleChain(j.token, j.denom, j.fee, j.nullifier, j.recipient);
        emit ChainRelease(j.burnBlockId, j.tipId, j.tipHash);
    }

    /// Same checks as `Vault._checkRelease`, through the public surface of `Vault`; the anchor check is the tip's.
    function _checkChainRelease(address token, uint256 denom, uint256 fee, bytes32 nullifier) private view {
        (, bool releasable,, bool releaseFrozen, uint256[] memory denoms,,) = this.tokenConfig(token);
        if (!releasable) revert UnsupportedToken();
        if (globalReleaseFrozen || releaseFrozen) revert ReleasesFrozen();
        bool known;
        for (uint256 i = 0; i < denoms.length; i++) {
            if (denoms[i] == denom) known = true;
        }
        if (!known || fee >= denom) revert InvalidDenomination();
        if (burnNullifierSpent[nullifier]) revert AlreadyReleased();
    }

    function _settleChain(address token, uint256 denom, uint256 fee, bytes32 nullifier, address recipient) private {
        (,,,,,, uint256 releaseCap) = this.tokenConfig(token);
        if (block.timestamp >= chainWindowStart[token] + capWindow) {
            chainWindowStart[token] = block.timestamp;
            chainReleasedInWindow[token] = 0;
        }
        if (chainReleasedInWindow[token] + denom > releaseCap) revert CapExceeded();
        chainReleasedInWindow[token] += denom;
        burnNullifierSpent[nullifier] = true;
        _payChain(token, recipient, denom - fee);
        if (fee != 0) _payChain(token, msg.sender, fee);
        emit Release(token, denom, nullifier, recipient, fee);
    }

    function _payChain(address token, address to, uint256 amount) private {
        if (token == NATIVE) {
            (bool ok,) = payable(to).call{value: amount}("");
            if (!ok) revert EthTransferFailed();
        } else {
            IERC20(token).safeTransfer(to, amount);
        }
    }
}
