# A struct's `comptime` member checks its body against its value
# annotation, as the pin's declaration does: a `Bool` neither is nor
# implicitly converts to an `Int`.
# expect: type mismatch for comptime member 'N': expected Int, found Bool
struct S:
    comptime N: Int = True


def main():
    print(S.N)
