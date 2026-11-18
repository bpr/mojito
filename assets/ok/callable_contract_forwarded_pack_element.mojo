# A generic callable contract naming a pack element by its own binder
# (`def[idx: Int]() -> Us[idx]`) accepts an alpha-equivalent function over
# the caller's pack once the call forwards that pack (`Us = *Ts`), both at
# a function's own pack and at `Tuple`'s.


def first[
    *Us: Movable & Defaultable & Deinitable, F: def[idx: Int]() -> Us[idx]
](f: F) -> Int:
    return 1


def through_function[*Ts: Movable & Defaultable & Deinitable]() -> Int:
    return first[*Ts](lambda [i: Int]() -> Ts[i]: Ts[i]())


def through_tuple[*Ts: Movable & Defaultable & Deinitable]() -> Int:
    var t = Tuple[*Ts](init_with=lambda [i: Int]() -> Ts[i]: Ts[i]())
    return len(t)


def main():
    print(through_function[Int, Bool]())
    print(through_tuple[Int, Bool]())
