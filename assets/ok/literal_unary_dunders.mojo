# A literal's unary `-` and `~` are its declared `IntLiteral.__neg__`,
# `__invert__`, and `FloatLiteral.__neg__`, whose signatures call dunders
# their structs declare below them, beside `__ceildiv__`; `+2.5` is the
# literal itself though `FloatLiteral` declares no `__pos__`.

comptime X = 12


def main():
    print(-4, ~4, (7).__ceildiv__(2), (2.5).__neg__())
    print(X.__neg__(), (7.5).__ceildiv__(2.0))
    print(+2.5, +3, -(2.5), -0.0, ~0, -(-3))
    comptime y = -2.5
    print(y)
