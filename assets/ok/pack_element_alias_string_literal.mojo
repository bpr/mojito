# A `comptime` alias of a pack element names the closed type of the
# argument that binds it: a `StringLiteral` element types its local as that
# literal's own type, not the open `StringLiteral[_]`.
def alias[*Ts: Writable & Copyable & ImplicitlyCopyable & Deinitable](*args: *Ts):
    comptime for i in range(len(Ts)):
        comptime T = Ts[i]
        var first: T = args[i]
        print(first)


def main():
    alias(1, "two", True)
    alias("three", 4.5)
