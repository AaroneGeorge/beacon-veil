// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

/// @title ILezTipAnchor
/// @notice Answers one question for `VaultChain.releaseChain`: is this LEZ block (id, hash) on the real, final chain?
/// @dev This is the one attested input of the chain route (docs/bridge/OUTBOUND.md). It is a plain fact about LEZ that
/// anyone can check with one RPC call (`getBlock(blockId)`), and the place where a Bedrock finality proof or a bonded
/// checkpoint with a challenge window goes in, without changing the guest or the vault. One anchor answers for one LEZ
/// chain. The vault holds its anchor as an immutable: a new anchor means a new vault (Security #8).
interface ILezTipAnchor {
    /// @return True when block `blockId` of the anchor's LEZ chain is accepted as final with hash `blockHash`.
    function isFinalTip(uint64 blockId, bytes32 blockHash) external view returns (bool);
}
