# A compile-time conditional whose arms are types evaluates to the type its
# condition selects, as at the pin: a `comptime` bound to it is that type,
# at module scope, as a struct member, and in a body, with equal or
# different arms.
comptime B = False
comptime Same = Int if True else Int
comptime T = String if B else Int


struct S:
    comptime U = Int if True else String


def main():
    comptime W = Int if not B else String
    var x: Same = 3
    var y: T = 4
    var u: S.U = 5
    var w: W = 6
    print(x, y, u, w)
