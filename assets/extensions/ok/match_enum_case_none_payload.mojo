# A `None` case pattern binds `Optional`'s and `OptionalReg`'s empty-tuple
# payload, which `_unsafe_get_union_payload[0]` vends from their storage, as
# every `UnionLike` case carries one under upstream's enum proposal: through
# a parameter, a `for` loop's reference, and beside `.None()` and `.None(_)`.
# `Optional` is `UnionLike` under upstream's enum proposal, whose
# parenthesized `None` the pin rejects (`docs/non-goals.md`).
def describe(value: Optional[Int]) -> String:
    match value:
        case .None(empty):
            return String("None with ", len(empty), " payload elements")
        case .Some(number):
            return String("Some(", number, ")")


def main():
    print(describe(Optional[Int]()))
    print(describe(Optional[Int](7)))
    for item in [Optional[Int](3), Optional[Int]()]:
        match item:
            case .None(empty):
                print("loop None", len(empty))
            case .Some(number):
                print("loop Some", number)
    var register = OptionalReg[Int]()
    match register:
        case .None(empty):
            print("register None", len(empty))
        case .Some(_):
            print("register Some")
    var absent = Optional[String]()
    match absent:
        case .None():
            print("None()")
        case _:
            pass
    match absent:
        case .None(_):
            print("None(_)")
        case _:
            pass
