# `Tuple.consume_elements` hands each element to a generic handler taken
# through upstream's closure-trait binder `F: def[idx: Int](var elt:
# Self.Ts[idx])`. Over a pack of one element type, `values.Ts[index]` folds
# to that type, so `len(element)` is `List.__len__` and the element is
# destroyed at the end of the handler, as the pin runs it.
def main():
    var values = ([1, 2, 3], [4, 5, 6])

    def print_length[index: Int](var element: values.Ts[index]):
        print(len(element))

    values^.consume_elements(print_length)
