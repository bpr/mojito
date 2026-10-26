# expect: does not match the signature required by trait 'Roundable'
# Upstream's `Roundable` requires both `__round__(self)` and
# `__round__(self, ndigits: Int)`, so a struct declaring only the first does
# not conform.
@fieldwise_init
struct R(Roundable):
    var v: Int

    def __round__(self) -> Self:
        return R(self.v)


def main():
    print(R(1).v)
