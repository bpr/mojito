# expect: cannot get payload for None case
# `Optional`'s payload getter asserts at compile time that the case it reads
# carries a payload, so its `None` case is matched `.None()` or `.None(_)`,
# which read none, and never bound.
# `Optional` is `UnionLike` under upstream's enum proposal, whose
# parenthesized `None` the pin rejects (`docs/non-goals.md`).
def main():
    var o = Optional[Int](3)
    match o:
        case .None(x):
            print(x)
        case _:
            pass
