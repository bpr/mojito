# `<<=` and `>>=` are augmented assignments: over a builtin integer they
# shift in place, and over a struct they call `__ilshift__`/`__irshift__`,
# as at the pin.


@fieldwise_init
struct Bits(ImplicitlyCopyable, Writable):
    var value: Int

    def __ilshift__(mut self, amount: Int):
        print("ilshift", amount)
        self.value = self.value << amount

    def __irshift__(mut self, amount: Int):
        print("irshift", amount)
        self.value = self.value >> amount


def main():
    var x = 8
    x >>= 1
    x <<= 2
    print(x)
    var n = 13
    while n > 0:
        print(n & 1, end=" ")
        n >>= 1
    print()
    var b: UInt8 = 3
    b <<= 6
    print(b)
    b >>= 7
    print(b)
    var s = Bits(5)
    s <<= 3
    s >>= 1
    print(s.value)
