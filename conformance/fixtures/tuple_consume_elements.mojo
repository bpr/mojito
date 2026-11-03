# `Tuple.consume_elements` takes its handler as a run-time argument through
# upstream's closure-trait binder; the deprecated `element_types` spelling of
# `Ts` still names the pack. Over a pack of one element type the handler's
# `values.element_types[index]` folds to `List[Int]`, so `len` resolves.
def main():
    var values = ([1, 2, 3], [4, 5, 6])

    def print_length[index: Int](var element: values.element_types[index]):
        print(len(element))

    values^.consume_elements(print_length)
