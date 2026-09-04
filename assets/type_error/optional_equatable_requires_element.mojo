# expect: not defined for Optional
# Optional's Equatable conformance is conditional on the element: comparing
# two Optionals of a non-Equatable struct rejects.
struct Opaque(Copyable, Movable):
    var value: Int

    def __init__(out self, value: Int):
        self.value = value

def main():
    var a = Optional[Opaque](Opaque(1))
    var b = Optional[Opaque](Opaque(1))
    print(a == b)
