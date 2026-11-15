# Nested overloaded operator folds: a run-time scalar operator over a call
# of an overloaded function folds its operands once, applying the overload
# the call resolved and its dunder to the operands already built, as
# upstream's emitter folds an operand from its resolved callee, so twelve
# levels of `f(...) + 1`, `f(-f(...))`, and `2.0 * f(...)` over four
# `def f` overloads check in time polynomial in their depth.


def f(x: Int) -> Int:
    return x


def f(x: Float64) -> Float64:
    return x


def f(x: String) -> String:
    return x


def f(x: Bool) -> Bool:
    return x


def main():
    var a = f(f(f(f(f(f(f(f(f(f(f(f(1) + 1) + 1) + 1) + 1) + 1) + 1) + 1) + 1) + 1) + 1) + 1) + 1
    print(a)
    var b = f(-f(-f(-f(-f(-f(-f(-f(-f(-f(-f(-f(-f(1)))))))))))))
    print(b)
    var c = 2.0 * f(2.0 * f(2.0 * f(2.0 * f(2.0 * f(2.0 * f(2.0 * f(2.0 * f(2.0 * f(2.0 * f(2.0 * f(2.0 * f(1.5))))))))))))
    print(c)
