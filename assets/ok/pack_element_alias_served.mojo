# A `comptime` alias of a pack element under a `comptime for` over the pack's
# length is the dependent `Ts[i]` it denotes: the template serves the body,
# whether the alias types a binding, constructs a value, or `Ts[i]` is
# spelled directly.
def build[*Ts: Movable & Writable & Deinitable & Defaultable]():
    comptime for i in range(len(Ts)):
        comptime T = Ts[i]
        var value: T = T()
        print(value)


def twice[*Ts: Writable & Copyable & ImplicitlyCopyable & Deinitable](*args: *Ts):
    comptime for i in range(len(Ts)):
        comptime T = Ts[i]
        var first: T = args[i]
        var second: T = first
        print(first, second)


def direct[*Ts: Writable & Copyable & ImplicitlyCopyable & Deinitable](*args: *Ts):
    comptime for i in range(len(Ts)):
        var first: Ts[i] = args[i]
        print(first)


def main():
    build[Int, String, Bool]()
    twice(1, "two", 3.5)
    direct("three", 4, False)
