# expect: field 'value' has non-'Deinitable' type 'T'
# A field typed by a bare struct type parameter needs a `Deinitable` bound
# (`Copyable & Movable` does not imply it) unless the struct declares a
# `Deinitable` conformance, conditionally or not.
struct Box[T: Copyable & Movable]:
    var value: Self.T

    def __init__(out self, var value: Self.T):
        self.value = value^

def main():
    var b = Box[Int](1)
    print(b.value)
