# expect: expected a tuple type to match against, got 'Int'
# Empty parentheses after a case are the empty tuple pattern, which a payload
# that is no tuple does not match; write `.Some(_)` to ignore it.
# `Optional` is `UnionLike` under upstream's enum proposal, where the pin
# asks for a payload pattern inside the parentheses (`docs/non-goals.md`).
def main():
    var o = Optional[Int](3)
    match o:
        case .Some():
            print("some")
        case _:
            pass
