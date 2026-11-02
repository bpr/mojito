# A cluster's group whose residual cases form a nested `hlcf.match`, inside
# an enclosing `match` case, completes the enclosing case when a residual's
# body runs. The pin crashed here until `1.3.0.dev2026110105`, which prints
# what Mojito prints.
@fieldwise_init
struct S(ImplicitlyCopyable):
    var v: Int

    def __eq__(self, other: Int) -> Bool:
        print("  eq", self.v, other)
        return self.v == other


def g(tag: String) -> Bool:
    print("  guard", tag)
    return False


def main():
    print("nested residual match then outer case")
    match S(1):
        case 1 if g("a"):
            print("A")
        case 1:
            print("B")
        case var x:
            print("C", x.v)
    print("nested residual match, value 2")
    match S(2):
        case 1 if g("a"):
            print("A")
        case 1:
            print("B")
        case var x:
            print("C", x.v)
    print("guard fails all of group")
    match S(1):
        case 1 if g("a"):
            print("A")
        case 2:
            print("two")
        case var x:
            print("C", x.v)
