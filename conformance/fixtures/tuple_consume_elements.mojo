# mojito-only (upstream regression at a79fbdf59f2, 2026-08-26): the head
# rejects its OWN `consume_elements` docstring example — `len(element)` on the
# dependent `values.Ts[index]` element type fails overload resolution
# ("cannot be converted ... 'Movable' is not a child trait of 'Sized'", with a
# malformed `Array[Int, Int(3)], Array[Int, Int(3)][SIMDLength(index)]`
# dependent type), under BOTH the canonical `Ts` and deprecated
# `element_types` spellings. Likely fallout of the list-literal -> Array
# retarget meeting the tightened generic-body checking. Mojito runs it and
# prints 3 / 3. Re-probe at the next re-pin; flip back to `run` when the head
# recovers.
def main():
    var values = ([1, 2, 3], [4, 5, 6])

    @parameter
    def print_length[index: Int](var element: values.element_types[index]):
        print(len(element))

    values^.consume_elements[print_length]()
