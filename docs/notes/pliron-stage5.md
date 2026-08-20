# Pliron Stage 5: Supported-Language Native Parity

Stage 5 grows the feature-gated Pliron backend one vertical slice at a time
until the native path has **zero exclusions** across Mojito's advertised
runnable subset (`assets/ok` + `assets/ownership_ok` fixtures with a `main`
entry), with the VM remaining the semantic oracle and canonical `.mir`
artifacts behaving identically through both paths. This note records the
design decisions and divergences per slice; the normative ABI contract stays
[`docs/native-abi.md`](../native-abi.md).

## Harness generalization and the capability matrix

- The Stage 4 exe manifest is renamed to the stage-neutral
  `conformance/pliron-parity.tsv` (`parity_exe_manifest_and_differential`);
  its schema and oracles are unchanged. Alongside the existing
  eligible-coverage floors, the guard set now ratchets the `excluded` count
  downward: each landed slice tightens the ceiling toward zero, so a
  regression cannot hide behind unrelated progress.
- `conformance/pliron-capability.tsv` is the roadmap-mandated generated
  capability matrix (`backend::pliron::capability::matrix`), with three
  sections: one row per textual-MIR instruction mnemonic, one per
  checked-type constructor spelling, and one per exported runtime symbol
  (`since` ABI versions from the `rt_abi` contract table). The instruction
  and type tables are pinned against the canonical schema vocabulary —
  `mir::text::INSTRUCTION_MNEMONICS` and the new `mir::text::TYPE_SPELLINGS`
  inventory — so adding a MIR instruction or `Ty` constructor fails the pin
  until it receives an explicit `supported`/`partial`/`unsupported` decision.
  `partial` rows state the lowered condition; `supported` rows may still
  reject malformed artifacts (untyped registers, missing blocks), which are
  verifier-level anomalies rather than capability gaps.
