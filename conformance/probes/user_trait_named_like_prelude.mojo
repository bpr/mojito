# PROBE: a user trait named like a prelude trait.
#
# The pin prints 1: the user's `Sized` shadows the prelude's in this module
# alone. Mojito agrees since `Sized` became the bundled library's prelude
# trait (R1051, closing R510); before, the user's declaration replaced the
# trait the bundled library's conformances name.
#
# Observed 2026-10-08 against `Mojo 1.2.0.dev2026092105 (e9569894)`.
#
# Run:    mojo run user_trait_named_like_prelude.mojo
#         cargo run -- run conformance/probes/user_trait_named_like_prelude.mojo
trait Sized:
    comptime size: Int

def main():
    print(1)
