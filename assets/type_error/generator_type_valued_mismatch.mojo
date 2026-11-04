# A type generator's application that is still a parameter (`G[Int]`) is
# its own type in the body, so a literal does not convert to it.
# expect: type mismatch for variable 'x': expected G[Int], found IntLiteral[5]
comptime Id[T: AnyType] = T


def g[G: __generator_type[T: AnyType] AnyType]():
    var x: G[Int] = 5


def main():
    g[Id]()
