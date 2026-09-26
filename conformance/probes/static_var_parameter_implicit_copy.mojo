# A static call passing a place to a `var` parameter copies it whatever its
# type, so Mojito accepts this and the VM then reports "double free of
# Pointer allocation". The pin rejects it: "value of type 'T' cannot be
# implicitly copied, it does not conform to 'ImplicitlyCopyable'". A method
# call's `var` argument demands the conformance; the static path does not.
struct Box[T: Copyable & Deinitable](Movable):
    var item: Self.T

    def __init__(out self, var item: Self.T):
        self.item = item^

    @staticmethod
    def keep(var v: Self.T) -> Int:
        return 1

    def via_static(self) -> Int:
        return Box.keep(self.item)


def main():
    print(Box[String]("s").via_static())
