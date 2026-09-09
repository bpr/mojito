# expect: expected String, List, or Tuple, found values.element_types[index]
# The handler's `var element: values.Ts[index]` (deprecated spelling
# `element_types`) is opaque under the pack's `Movable` bound inside the
# handler body, on both compilers: `len(element)` finds no `Sized`
# conformance (pinned Mojo a79fbdf59f2: "no matching function in call to
# 'len'"), and a handler that used nothing would abandon a non-`Deinitable`
# value instead.
def main():
    var values = ([1, 2, 3], [4, 5, 6])

    @__parameter
    def print_length[index: Int](var element: values.element_types[index]):
        print(len(element))

    values^.consume_elements[print_length]()
