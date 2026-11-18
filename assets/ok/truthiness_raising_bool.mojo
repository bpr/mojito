# A truthiness test is its operand's `x.__bool__()` call, so a raising
# `__bool__` raises out of every test position: a condition, `not`, a
# ternary, a `while`, and an `and`/`or` operand.


struct R(Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __bool__(self) raises -> Bool:
        if self.v < 0:
            raise Error("neg")
        return self.v != 0


def test(r: R) raises -> Int:
    if r:
        return 1
    return 0


def main() raises:
    print(test(R(2)), test(R(0)))
    try:
        _ = test(R(-1))
    except e:
        print("if:", e)
    var r = R(-1)
    try:
        print(not r)
    except e:
        print("not:", e)
    try:
        print(1 if r else 0)
    except e:
        print("ternary:", e)
    try:
        while r:
            pass
    except e:
        print("while:", e)
    try:
        print(r and True)
    except e:
        print("and:", e)
    var ok = R(3)
    print(not ok, 1 if ok else 0, ok and False, ok or False)
