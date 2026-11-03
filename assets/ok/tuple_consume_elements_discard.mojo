# A `Tuple.consume_elements` handler over a pack of one element type may
# discard its element (`_ = element^`): `values.Ts[index]` folds to
# `List[Int]`, which destroys implicitly. Over a mixed pack the element is
# opaque and abandoned (`tuple_deinit_with_abandoned_element`).
def main():
    var values = ([1, 2, 3], [4, 5, 6])

    def toss[index: Int](var element: values.Ts[index]):
        _ = element^

    values^.consume_elements(toss)
    print("done")
