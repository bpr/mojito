# PROBE: a pack forwarded into a method's own pack.
#
# **Differs.** The pin prints 1, two. Source validation checks `relay` from
# its template — the callee's pack binds to `a` whole, and an untaken
# `comptime if` arm after the call is reported — but the per-instantiation
# clone fails before running: the elaborator rewrites a whole-pack forwarding
# for a top-level `def` callee only (`comptime/mono.rs`,
# `top_level_whole_pack_forwarding_call`), so `self.take(*a)` reaches it as
# `not a compile-time value: 'a' is not a compile-time type`. Filed in
# `docs/roadmap.md` §1. When Mojito runs it, promote this file to
# `assets/ok/pack_forwarding_method.mojo` with its manifest rows.
#
# Observed 2026-09-22 against `Mojo 1.2.0.dev2026092105`.
#
# Run:    mojo run pack_forwarding_method_untaken_arm.mojo
#         cargo run -- run conformance/probes/pack_forwarding_method_untaken_arm.mojo
struct Sink:
    def __init__(out self):
        pass

    def take[*Ts: Writable](self, *a: *Ts):
        comptime for i in range(a.__len__()):
            print(a[i])

    def relay[*Ts: Writable](self, *a: *Ts):
        self.take(*a)


def main():
    Sink().relay(1, "two")
