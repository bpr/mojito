# A trait composition's operands are traits: `x` is an `Int` constant.
# expect: 'AnyTrait[Copyable]' does not implement the '__and__' method
def g[Tr: type_of(Movable)]() -> Int:
    return 1


def main():
    comptime x = 3
    comptime Bad = Copyable & x
    print(g[Bad]())
