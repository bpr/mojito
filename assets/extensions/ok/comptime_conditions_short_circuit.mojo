# A compile-time condition decides `and` and `or` left to right and stops
# once the left side decides them, and an `elif` past a taken arm is never
# decided: a union payload read guarded by its tag never runs over the other
# case, in a `comptime if` and in a `comptime match` case's tests and
# guard alike.
# `Optional` is `UnionLike` under upstream's enum proposal, whose members
# the pin does not declare (`docs/non-goals.md`).
def main():
    comptime o = Optional[Int](None)
    comptime if o._get_sum_discriminant() == 1 and o._unsafe_get_union_payload[1]() > 2:
        print("some big")
    else:
        print("else")
    comptime if o._get_sum_discriminant() == 0:
        print("none")
    elif o._unsafe_get_union_payload[1]() > 2:
        print("big")
    comptime match o:
        case .Some(x) if x > 2:
            print("big", x)
        case .Some(3):
            print("three")
        case _:
            print("other")
