# A `comptime if` condition that applies a function is a thunk the elaborator
# demands and runs: `is_even(n)` is lowered as `$comptime$parity$0` over the
# binder `n`, the instance is materialized with its callees, the fragment
# runs on the VM, and the result is cached by instance. A condition mixing a
# type binder and a value binder is decided the same way, and the taken arm
# may call another served `def`. A local `comptime` binding over a binder,
# chained or read beside a `comptime for` index, is read in the thunk as the
# parameter expression it denotes.
def is_even(n: Int) -> Bool:
    return n % 2 == 0


def at_least(n: Int, bound: Int) -> Bool:
    return n >= bound


def parity[n: Int]() -> String:
    comptime if is_even(n):
        return "even"
    else:
        return "odd"


def wide[T: Writable, n: Int](x: T) -> String:
    comptime if T == Int and at_least(n, 4):
        return "wide int " + String(x)
    elif at_least(n, 4):
        return "wide " + String(x)
    else:
        return "narrow " + String(x) + " " + parity[n]()


def shifted[n: Int]() -> String:
    comptime m = n + 1
    comptime p = m * 3
    comptime if is_even(p):
        return "shifted even"
    else:
        return "shifted odd"


def doubled[n: Int]():
    comptime m = n * 2
    comptime for i in range(3):
        comptime if is_even(m + i):
            print("doubled", n, i, "even")
        else:
            print("doubled", n, i, "odd")


def main():
    print(parity[4](), parity[7](), parity[4]())
    print(wide[Int, 8](1), wide[String, 4](String("s")), wide[Int, 3](2))
    print(shifted[1](), shifted[2]())
    doubled[1]()
    doubled[2]()
