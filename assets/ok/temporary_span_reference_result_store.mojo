# A store through a reference result whose call takes a temporary `Span`
# of the stored list: the temporary's anchor loan excludes nothing, so the
# write through the result runs, as at the pin: `5`.
def at[o: MutOrigin](s: Span[Int, o]) -> ref[o] Int:
    return s[0]


def main():
    var l: List[Int] = [1, 2]
    at(Span(l)) = 5
    print(l[0])
