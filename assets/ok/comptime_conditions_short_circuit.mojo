# A compile-time condition decides `and` and `or` left to right and stops
# once the left side decides them, and an `elif` past a taken arm is never
# decided: an enum payload read guarded by its tag never runs over the other
# variant, in a `comptime if` and in a `comptime match` case's tests and
# guard alike.
def main():
    comptime o = Optional[Int](None)
    comptime if o._get_enum_discriminant() == 1 and o._unsafe_get_enum_payload[1]() > 2:
        print("some big")
    else:
        print("else")
    comptime if o._get_enum_discriminant() == 0:
        print("none")
    elif o._unsafe_get_enum_payload[1]() > 2:
        print("big")
    comptime match o:
        case .Some(x) if x > 2:
            print("big", x)
        case .Some(3):
            print("three")
        case _:
            print("other")
