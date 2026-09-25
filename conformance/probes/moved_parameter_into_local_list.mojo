# PROBE (divergence): a `var` parameter of a parameter type moved into a local
# collection through a mutating method.
#
# `List.append` transfers what its argument carries into the receiver. A
# parameter whose type may carry loans stands for the caller's loans by its own
# place, so the erased generic body records a loan on `value` in `result`, and
# returning `result` after `value^` moved reads the moved parameter: "use of
# uninitialized value 'value'". The same store into a field of `self` works,
# because a destination rooted at a parameter installs nothing in this frame,
# and a non-generic body works because `String` carries no loans.
#
# Observed 2026-09-24 against `Mojo 1.2.0.dev2026092105 (e9569894)`:
#   mojo:   1
#   mojito: use of uninitialized value 'value'
#
# When fixed: promote to `assets/ok`.
struct Shelf[T: Copyable & Deinitable]:
    var n: Int

    def __init__(out self):
        self.n = 0

    def filled(self, var value: Self.T) -> List[Self.T]:
        var result = List[Self.T]()
        result.append(value^)
        return result^


def main():
    var s = Shelf[String]()
    print(len(s.filled("a")))
