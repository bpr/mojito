# A `Tuple.consume_elements` handler over a pack of one element type
# prints each element: `values.Ts[index]` folds to `List[Int]`, which is
# `Writable`.
def main():
    var values = ([1, 2, 3], [4, 5, 6])

    def show[index: Int](var element: values.Ts[index]):
        print(element)

    values^.consume_elements(show)
