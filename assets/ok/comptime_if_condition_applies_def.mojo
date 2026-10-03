# A `comptime if` condition that applies a function is a thunk the elaborator
# demands and runs: `is_even(n)` is lowered as `$comptime$parity$0` over the
# binder `n`, the instance is materialized with its callees, the fragment
# runs on the VM, and the result is cached by instance. A condition mixing a
# type binder and a value binder is decided the same way, and the taken arm
# may call another served `def`.
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


def main():
    print(parity[4](), parity[7](), parity[4]())
    print(wide[Int, 8](1), wide[String, 4](String("s")), wide[Int, 3](2))
