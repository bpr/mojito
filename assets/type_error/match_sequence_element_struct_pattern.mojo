# expect: against pattern type 'P'
# A sequence element is untyped, so a struct pattern cannot match it.
@fieldwise_init
struct P(Copyable):
    var x: Int


def main():
    var xs: List[P] = [P(1)]
    match xs:
        case [P(x=q)]:
            print(q)
        case _:
            print("other")
