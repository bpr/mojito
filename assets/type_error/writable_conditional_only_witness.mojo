# expect: struct 'Box' method 'write_to' does not match the signature required by trait 'Writable'
# An unconditional `Writable` conformance needs an unconditional `write_to`:
# a witness under a `where` clause does not satisfy it, as at the pin.


struct Box[T: Copyable & Deinitable](Writable):
    var v: Self.T

    def __init__(out self, v: Self.T):
        self.v = v.copy()

    def write_to(self, mut writer: Some[Writer]) where conforms_to(Self.T, Writable):
        writer.write("Box!")


def main():
    print(Box[Int](1))
