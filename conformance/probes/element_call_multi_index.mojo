# PROBE (dispatch confirmation): does the audited head dispatch the bare
# MULTI-INDEX element-call spelling `g[1, 1](10)` as variadic subscript
# followed by the element call?
#
# Context: Mojito's element-call re-dispatch maps every runtime-value bracket
# group onto the subscript before dispatching the element. The single-index
# form was differentially confirmed (the former recorded subset gap); the
# multi-index form is inferred from the same rule and needs one upstream
# confirmation.
#
# Confirmed 2026-08-18 against the pinned `ae386d1b204` build: prints 40.
#
# Run:    mojo run element_call_multi_index.mojo
#         cargo run -- run conformance/probes/element_call_multi_index.mojo
#
# If Mojo accepts and prints 40: no action — the shapes match.
# If Mojo rejects: gate the multi-index arm of the re-dispatch (keep the
# single-index base) and record the divergence in parity.tsv.
@fieldwise_init
struct Doubler(def(Int) -> Int, Copyable, ImplicitlyCopyable):
    var gain: Int

    def __call__(self, x: Int) -> Int:
        return x * self.gain

struct Grid(Copyable):
    var cells: List[Doubler]

    def __init__(out self, var cells: List[Doubler]):
        self.cells = cells^

    def __getitem__(self, row: Int, column: Int) -> Doubler:
        return self.cells[row * 2 + column]

def main():
    var g: Grid = Grid([Doubler(1), Doubler(2), Doubler(3), Doubler(4)])
    print(g[1, 1](10))
