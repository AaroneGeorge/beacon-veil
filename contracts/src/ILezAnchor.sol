// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

/// @title ILezAnchor
/// @notice Answers one question for `Vault.release`: is this LEZ burn-accumulator root final for this bridge instance?
/// @dev Stage 1 of the outbound design (docs/bridge/OUTBOUND.md) is an optimistic Bedrock checkpoint with a challenge
/// window. The sprint ships only `StubLezAnchor`, an owner-posted stub. A real anchor replaces it by deploying a new
/// vault (Security #8): the vault holds its anchor as an immutable.
interface ILezAnchor {
    /// @param lezInstance The bridge instance id on LEZ (32 bytes, the instance PDA).
    /// @param accumulatorRoot Root of that instance's burn accumulator.
    /// @return True when `accumulatorRoot` is accepted as final for `lezInstance`.
    function isFinal(bytes32 lezInstance, bytes32 accumulatorRoot) external view returns (bool);
}
