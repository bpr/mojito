# PROBE: a pack forwarded into `print`.
#
# **Differs.** The pin prints `1 two`. Source validation checks `outer` from
# its template — a forwarded pack whose bound gives `Writable` is one `print`
# argument, and an untaken `comptime if` arm after the call is reported —
# but the per-instantiation clone fails before running: the elaborator
# rewrites a whole-pack forwarding for a top-level `def` callee only, so the
# clone's `print(*a)` reaches the executable check as `call spread outside a
# specialized type pack`. Filed in `docs/roadmap.md` §1. When Mojito runs it,
# promote this file to `assets/ok/pack_forwarding_print.mojo` with its
# manifest rows.
#
# Observed 2026-09-22 against `Mojo 1.2.0.dev2026092105`.
#
# Run:    mojo run pack_forwarding_print_untaken_arm.mojo
#         cargo run -- run conformance/probes/pack_forwarding_print_untaken_arm.mojo
def outer[*Ts: Writable](*a: *Ts):
    print(*a)


def main():
    outer(1, "two")
