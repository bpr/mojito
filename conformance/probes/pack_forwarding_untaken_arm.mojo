# PROBE: a body that forwards its pack to another callee.
#
# **Differs.** The pin checks `outer` from its template and rejects the
# untaken arm: "cannot implicitly convert 'StringLiteral["bad"]' value to
# 'Int'". Mojito has no symbolic rule for a call that spreads a pack which is
# still a parameter, so `outer` gets no verdict from source validation
# (`templates.pack_no_verdict` in `--timings`), only the taken arm is checked
# per instantiation, and the program prints 1, two. Filed in `docs/roadmap.md`
# §1. When Mojito rejects it, promote this file to
# `assets/type_error/untaken_comptime_if_pack_forwarding.mojo` with a row in
# `conformance/assets-mojo-errors.tsv`.
#
# Observed 2026-09-20 against `Mojo 1.1.0.dev2026082605 (dd957314)`.
#
# Run:    mojo run pack_forwarding_untaken_arm.mojo
#         cargo run -- run conformance/probes/pack_forwarding_untaken_arm.mojo
def inner[*Ts: Writable](*a: *Ts):
    comptime for i in range(a.__len__()):
        print(a[i])


def outer[*Ts: Writable](*a: *Ts):
    inner(*a)
    comptime if 1 > 2:
        var x: Int = "bad"


def main():
    outer(1, "two")
