# An annotation whose head names a compile-time value is that value's
# subscript: a `TypeList` value's element, local or module-scope, at a
# literal or a `comptime` index, and any struct value's `__getitem_param__`.
# The same subscript is a type in a bracket argument.
comptime TL = TypeList.of[Trait=AnyType, Bool, Int]()


struct Pick:
    comptime __getitem_param__[i: Int] = Int

    def __init__(out self):
        pass


comptime picked = Pick()


def main():
    comptime tl = TypeList.of[Trait=AnyType, Bool, Int]()
    comptime n = 1
    var a: tl[1] = 3
    var b: tl[n] = 4
    var c: tl[0] = True
    var d: TL[1] = 5
    print(a, b, c, d)

    var l = List[tl[1]]()
    l.append(6)
    print(len(l), tl[1] == Int)

    comptime p = Pick()
    var e: p[0] = 7
    var f: picked[1] = 8
    print(e, f)
