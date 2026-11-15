# expect: expected a tuple type to match against, got 'Int'
# Several subpatterns after a case form one tuple pattern over the payload,
# which a payload that is no tuple does not match.
# `Optional` is `UnionLike` under upstream's enum proposal, where the pin
# takes at most one payload subpattern (`docs/non-goals.md`).
def main():
    var o = Optional[Int](3)
    match o:
        case .Some(x, y):
            print(x, y)
        case _:
            pass
