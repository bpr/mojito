# Phase 4: `Comparable` turns `<`/`<=`/`>`/`>=` into an ordering contract for an
# opaque type parameter. `& Copyable` lets the helpers return an explicit copy
# of a borrowed `T` under the move-only rule.
def min_value[T: Comparable & Copyable](a: T, b: T) -> T:
    if b < a:
        return b.copy()
    return a.copy()

def clamp[T: Comparable & Copyable](x: T, lo: T, hi: T) -> T:
    if x < lo:
        return lo.copy()
    if x > hi:
        return hi.copy()
    return x.copy()

def main():
    print(min_value(3, 5))
    print(min_value(9, 2))
    print(clamp(10, 0, 7))
    print(clamp(-3, 0, 7))
    print(min_value(2.5, 1.5))
