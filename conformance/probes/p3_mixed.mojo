# The mixed-feature probe every P3 entry of docs/parametric-mir-plan.md
# extends, so migrations that pass alone also compose. Each P3 step adds the
# construct it moves to MIR, inside the bodies already here.
#
# P3a (2026-10-03): a `comptime if` on a value binder and on a type binder in
# one template-served `def`, a region inside a `try`, a condition that
# applies a function (a thunk the elaborator runs), and a generic struct
# instance built in the taken arm. Expected output, VM and native:
#   int 4 | other 2.5 | box 7
#   caught big | 2
#   even odd
#
# P3b (2026-10-03): a `comptime for` over the value binder beside the region,
# with a `comptime if` over its index, a loop nested over the index, and a
# compile-time `continue`, in the same template-served `def`. Expected
# output, VM and native:
#   0 0 3 | 1 4 9
#
# P3c adds a `SIMD[dt, n]` body; P3d a value-keyed struct; P3e a method with
# its own binders and a nested `def`.
@fieldwise_init
struct Box[T: Copyable & Deinitable & Writable](Copyable, Movable):
    var v: Self.T


def is_even(n: Int) -> Bool:
    return n % 2 == 0


def describe[T: Writable, n: Int](x: T) -> String:
    comptime if T == Int and n > 2:
        return "int " + String(n)
    elif n > 2:
        return "other " + String(x)
    else:
        return "box " + String(Box[Int](7).v)


def guarded[n: Int]() raises -> Int:
    try:
        comptime if n > 2:
            raise Error("big")
        return n
    except e:
        print("caught", e)
        return -1


def parity[n: Int]() -> String:
    comptime if is_even(n):
        return "even"
    else:
        return "odd"


def steps[n: Int]() -> String:
    var out = String("")
    comptime for i in range(n + 1):
        comptime if i == 2:
            continue
        var sum = 0
        comptime for j in range(i):
            sum += j
        out += String(sum) + " "
    out += "|"
    comptime for i in range(1, n + 1):
        out += " " + String(i * i)
    return out


def main() raises:
    print(describe[Int, 4](1), "|", describe[Float64, 4](2.5), "|", describe[Int, 1](0))
    print(guarded[5](), "|", guarded[2]())
    print(parity[4](), parity[7]())
    print(steps[3]())
