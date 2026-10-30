# A chained comparison's middle operand is evaluated once and converted per
# link: `0 <= n < 2 * w` converts `n: SIMDLength` to `Int` for `Int.__le__`
# and passes it unconverted to `SIMDLength.__lt__`; a literal middle is
# `Float64` for one link and `Int` for the next.
struct Lanes[length: SIMDLength]:
    @staticmethod
    def check[m: SIMDLength]():
        comptime assert 0 <= m < 2 * Self.length, "mask out of range"
        print("in range")

def in_mask(n: SIMDLength) -> Bool:
    return 0 <= n < 2 * SIMDLength(2)

def main():
    print(in_mask(SIMDLength(3)))
    print(in_mask(SIMDLength(5)))
    var s = SIMDLength(3)
    print(1 < s <= 3 < 4)
    print(s < 4 <= 7)
    var x = 0.5
    var y = 3
    print(x < 1 < y)
    comptime ok = 0 <= SIMDLength(1) < 2 * SIMDLength(2)
    print(ok)
    Lanes[SIMDLength(4)].check[SIMDLength(7)]()
