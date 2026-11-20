// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

import {Ownable} from "@openzeppelin/contracts/access/Ownable.sol";
import {ILezTipAnchor} from "./ILezTipAnchor.sol";

/// @title StubLezTipAnchor (STUB: owner-attested, trusted)
/// @notice THIS IS A STUB. The owner attests LEZ block (id, hash) pairs and they become final after `delay` seconds.
/// Whoever holds the owner key can make any hash final, including one of a chain the owner built from the pinned
/// block, so the owner is trusted for this one statement: "block `blockId` with hash `blockHash` is on the real,
/// final LEZ chain". It no longer posts any bridge-specific value (no accumulator root): the burn itself is proven
/// from block bytes by guest `burn_chain`. It stands in for a Bedrock finality proof or a bonded checkpoint with a
/// challenge window (docs/bridge/OUTBOUND.md, docs/bridge/TRUST.md), which is milestone work.
contract StubLezTipAnchor is ILezTipAnchor, Ownable {
    /// @notice Seconds between attesting a tip and its acceptance. Stands in for the challenge window.
    uint64 public immutable delay;

    /// @notice blockId => blockHash => time attested (0 = never).
    mapping(uint64 => mapping(bytes32 => uint64)) public attestedAt;

    event TipAttested(uint64 indexed blockId, bytes32 indexed blockHash, uint64 finalAt);

    error AlreadyAttested();

    constructor(address owner_, uint64 delay_) Ownable(owner_) {
        delay = delay_;
    }

    /// @notice Owner attests a block read from LEZ (`getBlock(blockId)`).
    function attestTip(uint64 blockId, bytes32 blockHash) external onlyOwner {
        if (attestedAt[blockId][blockHash] != 0) revert AlreadyAttested();
        uint64 nowTs = uint64(block.timestamp);
        attestedAt[blockId][blockHash] = nowTs;
        emit TipAttested(blockId, blockHash, nowTs + delay);
    }

    /// @inheritdoc ILezTipAnchor
    function isFinalTip(uint64 blockId, bytes32 blockHash) external view returns (bool) {
        uint64 t = attestedAt[blockId][blockHash];
        return t != 0 && block.timestamp >= uint256(t) + delay;
    }
}
