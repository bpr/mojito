# expect: 'element' abandoned without being explicitly destroyed: unhandled explicitly destroyed type 'AnyType'
# Discarding the transferred element (`_ = element^`) destroys it implicitly,
# which a `Movable`-only value cannot be: it is abandoned, as upstream.
def main():
    var values = ([1, 2, 3], [4, 5, 6])

    @__parameter
    def toss[index: Int](var element: values.Ts[index]):
        _ = element^

    values^.consume_elements[toss]()
