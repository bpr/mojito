# expect: unpack is only supported when the callee accepts a variadic pack
# A callee without a positional collector cannot take a forwarded pack, even
# when its arity would match one instantiation.
def two(x: Int, y: String):
    print(x, y)


def outer[*Ts: Writable](*a: *Ts):
    two(*a)


def main():
    outer(1, "two")
