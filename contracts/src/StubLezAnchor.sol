// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

import {Ownable} from "@openzeppelin/contracts/access/Ownable.sol";
import {ILezAnchor} from "./ILezAnchor.sol";

/// @title StubLezAnchor (STUB: owner-posted, trusted)
/// @notice THIS IS A STUB. The owner posts LEZ burn-accumulator roots and they become final after `delay` seconds.
/// Whoever holds the owner key can make any root final, so the owner is trusted for release. It stands in for the
/// stage-1 optimistic Bedrock anchor described in docs/bridge/OUTBOUND.md and docs/bridge/TRUST.md, which is
/// milestone work, not sprint work.
contract StubLezAnchor is ILezAnchor, Ownable {
    /// @notice Seconds between posting a root and its acceptance. Stands in for the challenge window.
    uint64 public immutable delay;

    /// @notice lezInstance => accumulatorRoot => time posted (0 = never).
    mapping(bytes32 => mapping(bytes32 => uint64)) public postedAt;

    event RootPosted(bytes32 indexed lezInstance, bytes32 indexed accumulatorRoot, uint64 finalAt);

    error AlreadyPosted();

    constructor(address owner_, uint64 delay_) Ownable(owner_) {
        delay = delay_;
    }

    /// @notice Owner posts a root read from the public state of the LEZ burn accumulator.
    function post(bytes32 lezInstance, bytes32 accumulatorRoot) external onlyOwner {
        if (postedAt[lezInstance][accumulatorRoot] != 0) revert AlreadyPosted();
        uint64 nowTs = uint64(block.timestamp);
        postedAt[lezInstance][accumulatorRoot] = nowTs;
        emit RootPosted(lezInstance, accumulatorRoot, nowTs + delay);
    }

    /// @inheritdoc ILezAnchor
    function isFinal(bytes32 lezInstance, bytes32 accumulatorRoot) external view returns (bool) {
        uint64 t = postedAt[lezInstance][accumulatorRoot];
        return t != 0 && block.timestamp >= uint256(t) + delay;
    }
}
