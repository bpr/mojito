# Neighbouring cases whose first command tests a spelled literal on the
# same path form a cluster, as upstream's `findClusterSize` /
# `emitCaseCluster` emit them: the subject's `__eq__` runs once per
# distinct spelling, in spelling order (bytewise, quotes kept), and the
# first spelling that compares equal selects its cases, which then run in
# case order. A trailing `_` is the cluster's else arm. An `== expr` after a
# spelled literal joins the cluster under the empty spelling, which sorts
# first; a run led by `== expr` or by a binding is not clustered.
@fieldwise_init
struct S(ImplicitlyCopyable):
    var v: Int

    def __eq__(self, other: Int) -> Bool:
        print("  eq", self.v, other)
        return self.v == other


@fieldwise_init
struct W(ImplicitlyCopyable):
    var s: S


def g(tag: String) -> Bool:
    print("  guard", tag)
    return False


def expected(v: Int) -> Int:
    print("  expected", v)
    return v


def main():
    print("spellings, not values")
    match 4:
        case 4:
            print("dec")
        case 0x4:
            print("hex")
    match 1.5:
        case 1.50:
            print("1.50")
        case 1.5:
            print("1.5")
    match String("b"):
        case "b":
            print("double")
        case "a":
            print("a")
        case 'b':
            print("single")
    print("a failed residual leaves the cluster")
    match 4:
        case 0x4 if g("hex"):
            print("hex")
        case 4:
            print("dec")
    print("one test per spelling")
    match S(1):
        case 2:
            print("two")
        case 1:
            print("one")
    match S(1):
        case 1 if g("a"):
            print("one-a")
        case 2:
            print("two")
        case 1:
            print("one-b")
    match S(3):
        case 2:
            print("two")
        case 1:
            print("one")
        case _:
            print("other")
    print("a binding ends the run")
    match S(5):
        case 1:
            print("one")
        case var x if x.v > 4:
            print("big")
        case 5:
            print("five")
    print("== expr joins under the empty spelling")
    match S(2):
        case 1:
            print("one")
        case == expected(2):
            print("expected")
        case 3:
            print("three")
    print("== expr leads: case order")
    match S(2):
        case == expected(2):
            print("expected")
        case 1:
            print("one")
    print("fields")
    match W(S(3)):
        case W(s=3):
            print("three")
        case W(s=2):
            print("two")
    print("tuple elements cluster level by level")
    match (S(1), S(2)):
        case (2, 1):
            print("21")
        case (1, 3):
            print("13")
        case (1, 2):
            print("12")
        case (2, 2):
            print("22")
        case _:
            print("other")
    var t = (S(0), S(2))
    print("a leading binding")
    match t:
        case (x, 3):
            print("3", x.v)
        case (y, 1):
            print("1", y.v)
        case (z, 2):
            print("2", z.v)
    print("a leading wildcard")
    match t:
        case (_, 3):
            print("3")
        case (_, 1):
            print("1")
        case (_, 2):
            print("2")
