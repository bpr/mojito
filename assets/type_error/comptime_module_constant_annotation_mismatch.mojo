# A module constant's annotation converts its initializer as a `var`'s does:
# an `Int32` value has no implicit conversion to `Bool`.
# expect: type mismatch for comptime 'K': expected Bool, found Int32
comptime K: Bool = Int32(4)


def main():
    print(K)
