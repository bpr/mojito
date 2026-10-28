# expect: type mismatch for operand of 'or'
# `and`/`or` operands with no common type each convert to `Bool`, so an
# operand whose type does not implement `__bool__` is rejected.
@fieldwise_init
struct NoTruth(ImplicitlyCopyable):
    var v: Int


def main():
    var a = NoTruth(1)
    print((a or 2) == True)
