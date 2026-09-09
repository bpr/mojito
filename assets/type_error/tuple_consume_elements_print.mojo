# expect: does not conform to trait 'Writable'; either prove the conformance with 'conforms_to', or add conformance
# The handler's element is opaque under the pack's `Movable` bound, so
# printing it reports upstream's `Writable` conformance failure.
def main():
    var values = ([1, 2, 3], [4, 5, 6])

    @__parameter
    def show[index: Int](var element: values.Ts[index]):
        print(element)

    values^.consume_elements[show]()
