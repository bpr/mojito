# PROBE (re-probe): what licenses a trait use of a pack element under a
# symbolic index.
#
# Both compilers print 1 two / 1 two / 1 two. The pin accepts a bound on the
# pack declaration, a method `where conforms_to(Self.Ts.values, Trait)`, and
# a method `where Self.Ts.all_conforms_to[Trait]()`. It rejects a use backed
# only by a struct-header conditional conformance
# (`pack_element_header_conformance_only.mojo`), by a disjunctive `where`
# (`pack_element_where_disjunction.mojo`), or by nothing: "an element of
# 'values' with type 'Ts.values[...]' does not conform to trait 'Writable';
# either prove the conformance with 'conforms_to', or add conformance".
#
# Observed 2026-09-20 against `Mojo 1.1.0.dev2026082605 (dd957314)`.
#
# Run:    mojo run pack_element_bound_source.mojo
#         cargo run -- run conformance/probes/pack_element_bound_source.mojo
struct Bag[*Ts: Movable](
    Deinitable where Ts.all_conforms_to[Deinitable](),
    Movable,
):
    var storage: Tuple[*Self.Ts]

    def __init__(out self, var *args: *Self.Ts):
        self.storage = Tuple[*Self.Ts](*args^)

    def show_conforms(self) where conforms_to(Self.Ts.values, Writable):
        comptime for i in range(Self.Ts.length):
            print(self.storage[i])

    def show_all(self) where Self.Ts.all_conforms_to[Writable]():
        comptime for i in range(Self.Ts.length):
            print(self.storage[i])


def show[*Ts: Writable](*a: *Ts):
    comptime for i in range(a.__len__()):
        print(a[i])


def main():
    var b = Bag[Int, String](1, "two")
    b.show_conforms()
    b.show_all()
    show(1, "two")
