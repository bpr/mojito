# expect: type mismatch for value parameter 'n'
# A struct's type member is no value: passed where `Box` takes its `Int`
# parameter, it is rejected, while a value member passes.
struct Box[n: Int]:
    comptime w = Self.n * 10


struct S:
    comptime B = Int
    comptime m = Box[Self.B].w


def main():
    print(S.m)
