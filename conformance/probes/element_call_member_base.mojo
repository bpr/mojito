# PROBE (dispatch confirmation): does the audited head dispatch the bare
# MEMBER-BASE element-call spelling `h.items[0](5)` as subscript-then-call,
# exactly like the identifier-base `objs[0](3)` it accepts?
#
# Context: Mojito's element-call re-dispatch covers identifier, member, and
# multi-index bases. The identifier base was differentially confirmed (the
# former recorded subset gap); the member base is inferred from the same
# uniform subscript-then-call rule and needs one upstream confirmation.
#
# Confirmed 2026-08-18 against the pinned `ae386d1b204` build: prints 15.
#
# Run:    mojo run element_call_member_base.mojo
#         cargo run -- run conformance/probes/element_call_member_base.mojo
#
# If Mojo accepts and prints 15: no action — the shapes match.
# If Mojo rejects: gate the member-base arm of the re-dispatch (keep the
# identifier base) and record the divergence in parity.tsv.
@fieldwise_init
struct Doubler(def(Int) -> Int, Copyable, ImplicitlyCopyable):
    var gain: Int

    def __call__(self, x: Int) -> Int:
        return x * self.gain

@fieldwise_init
struct Holder(Copyable):
    var items: List[Doubler]

def main():
    var h: Holder = Holder([Doubler(3)])
    print(h.items[0](5))
