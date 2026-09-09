# expect: 'element' abandoned without being explicitly destroyed: unhandled explicitly destroyed type 'AnyType'
# A `Tuple.deinit_with` (`consume_elements`) handler's element is opaque
# under the pack's `Movable` bound: it does not prove `Deinitable`, so a
# handler that does not explicitly destroy it abandons it (as upstream).
@fieldwise_init
struct Res(Movable):
    var id: Int


def main():
    var t = ([Res(5)], 6)

    @__parameter
    def toss[index: Int](var element: t.element_types[index]):
        pass

    t^.deinit_with[toss]()
    print("done")
