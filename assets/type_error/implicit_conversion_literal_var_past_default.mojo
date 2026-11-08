# expect: expected F, found IntLiteral[2]
# An integer literal materializes to `Int` before an `@implicit`
# constructor: one taking `Float64` would need a second conversion.
struct F(ImplicitlyCopyable):
    var v: Float64

    @implicit
    def __init__(out self, value: Float64, /):
        self.v = value + 0.5


def main():
    var y: F = 2
    print(y.v)
