# expect: unqualified access to struct parameter 'T'; use 'Self.T' instead
# A struct's own parameter is spelled `Self.T` inside its body; the bare
# spelling is upstream's error.
struct Box[T: Copyable & Movable & Deinitable]:
    var value: T

    def __init__(out self, var value: Self.T):
        self.value = value^

def main():
    var b = Box[Int](1)
    print(b.value)
