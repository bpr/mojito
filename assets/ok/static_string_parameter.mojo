# A `StaticString` compile-time parameter: a literal argument converts
# through `StringSpan`'s `@implicit` literal constructor, which the
# elaborator runs, and the bound view reads back at run time.


def name[s: StaticString]() -> String:
    return String(s)


def width[s: StaticString]() -> Int:
    return s.byte_length()


def is_a[s: StaticString]() -> Bool:
    comptime if s == "a":
        return True
    else:
        return False


struct Tag[s: StaticString]:
    @staticmethod
    def get() -> String:
        return String(Self.s)


def main():
    print(name["param"]())
    print(name["a"](), name["bc"]())
    comptime n = width["param"]()
    print(n)
    print(Tag["xyz"].get())
    print(is_a["a"](), is_a["b"]())
    print(SIMD[DType.int8, width["abcd"]()](1))
