// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

import {IRiscZeroVerifier} from "risc0/IRiscZeroVerifier.sol";

/// @title Vault (freeze-1 stub, owner W5)
/// @notice Fixed-denomination ETH and ERC-20 deposits into an incremental SHA-256 tree; release behind a RISC Zero
/// proof and an `ILezAnchor`. PLAN.md section 2.4. This stub only fixes the constructor shape.
contract Vault {
    IRiscZeroVerifier public immutable verifier;

    constructor(IRiscZeroVerifier verifier_) {
        verifier = verifier_;
    }
}
