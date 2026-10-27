# A `comptime if` over a symbolic struct's conditional conformance is decided
# per instance, not folded false in the generic body.
struct Maybe[T: Copyable & Deinitable](Writable where conforms_to(T, Writable)):
    var v: Self.T

    def __init__(out self, var v: Self.T):
        self.v = v^

    def write_to(self, mut w: Some[Writer]):
        comptime assert conforms_to(Self.T, Writable)
        w.write("Maybe")

@fieldwise_init
struct Plain(Copyable):
    var x: Int

def f[T: Copyable & Deinitable]():
    comptime if conforms_to(Maybe[T], Writable):
        print("writable")
    else:
        print("not writable")

def main():
    f[Int]()
    f[Plain]()
