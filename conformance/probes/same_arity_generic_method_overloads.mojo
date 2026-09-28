# PROBE (divergence): two same-arity generic overloads of one method that a
# call's type arguments specialize alike.
#
# The pinned Mojo ranks `pick(a: T, b: Int)` above `pick(a: T, b: T)` for
# `pick(2, 3)` and prints `2`. Mojito rejects the program: both overloads
# specialize to one clone symbol, which is declared twice.
#
# Observed 2026-09-28 against `Mojo 1.2.0.dev2026092105 (e9569894)`:
#   mojo:   2
#   mojito: type error ("'pick$y3:Int' is already declared in this scope")
#
# When fixed: promote to `assets/ok`.
@fieldwise_init
struct First:
    var tag: Int

    def pick[T: Copyable](self, a: T, b: T) -> T:
        return b.copy()

    def pick[T: Copyable](self, a: T, b: Int) -> T:
        return a.copy()


def main():
    print(First(0).pick(2, 3))
