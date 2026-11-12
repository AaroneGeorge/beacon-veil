# K9 part 2 on the final vault v3 against run B (2026-11-12 14:20-14:21 UTC)

- Vault v3 0x380be74d9F3dFeBAaFC5F911CD6A45bFbeE3aD27 on Sepolia: estop 3.1.0, burn image 0xf929f9f4…97cf.
- Light client: run B instance 3YmV7VDujQF7zqtfMLf9VMKc4G9Vg1qYwYiTFXmJ8kBX, attest D4EHJuxj… from release set bf95e359. The followed records were only read.
- Flow:
  1. attest-root (public), attesting the vault's deposit root via a storage proof plus the gloas RLP header.
  2. Private claim (dev mode), LEZ tx 48e096c4….
  3. Repeat claim refused with ESA:E11_ALREADY_CONSUMED.
- Deposit openings and wallet env are deliberately NOT copied here. They live in the operator's run dir.
