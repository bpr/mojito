# `Bool`'s `&`, `|`, `^`, and `~`, eager over two `Bool`s, in place
# (`&=`, `|=`, `^=`), and folded in a parameter expression.


def f[a: Bool, b: Bool]() -> Bool:
    return a & b


struct Tag[c: Bool]:
    @staticmethod
    def show():
        print(Self.c)


def main():
    var a = True
    var b = False
    print(a & b, a | b, a ^ b, ~a)
    a &= b
    print(a)
    b |= True
    print(b)
    b ^= True
    print(b)
    print(f[True, True](), f[True, False]())
    comptime k = True ^ False
    print(k)
    Tag[True & False].show()
    Tag[True | False].show()
