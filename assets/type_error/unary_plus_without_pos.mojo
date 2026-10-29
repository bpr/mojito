# Unary `+x` is the call `x.__pos__()`, so a struct without one rejects it.
# expect: operator '+' is not defined for M
@fieldwise_init
struct M:
    var x: Int


def main():
    var m = M(1)
    var p = +m
