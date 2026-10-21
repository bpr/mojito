# Coverage spans enums and products: an `Optional`'s constructors and the
# Cartesian cells of a tuple or struct of closed leaves, each covered only
# by a pattern matching all of it (a payload test refines, a guard adds
# nothing). A covered match satisfies return and initialization analysis,
# and a case earlier ones cover warns and is never reached. A subject with
# an open leaf (`Int`) only finds repeated root literal spellings, so
# `0x4` is no repeat of `4`. Each shape is its own function.
@fieldwise_init
struct P(Copyable, Movable):
    var a: Bool
    var b: Bool


def optional_returns(o: Optional[Int]) -> Int:
    match o:
        case .None:
            return 0
        case .Some(x):
            return x


def bool_alternatives_return(b: Bool) -> Int:
    match b:
        case True | False:
            return 1


def optional_initializes(o: Optional[Int]) -> Int:
    var y: Int
    match o:
        case .None:
            y = 0
        case .Some(v):
            y = v
    return y


def payload_refines(o: Optional[Int]):
    match o:
        case .Some(0):
            print("some zero")
        case .None:
            print("none")


def payload_after_whole(o: Optional[Int]):
    match o:
        case .Some(_):
            print("some")
        case .Some(0):
            print("some zero")
        case .None:
            print("none")


# Never called: the pin crashes at run time on a `_` after `Optional` cases
# that cover it, though it warns as Mojito does.
def catch_all_after_optional_cases(o: Optional[Int]):
    match o:
        case .Some(_):
            print("some")
        case .None:
            print("none")
        case _:
            print("after")


def repeated_payload_binding(o: Optional[Int]):
    match o:
        case .Some(x):
            print("x", x)
        case .Some(y):
            print("y", y)


def optional_and_bool(o: Optional[Int], b: Bool):
    match (o, b):
        case (.Some(_), True):
            print("some, true")
        case (.None, _):
            print("none")
        case (.Some(1), False):
            print("one, false")


def struct_product(p: P):
    match p:
        case P(a=True):
            print("a")
        case P(b=False):
            print("b")


def open_leaf_product(t: Tuple[Bool, Int]):
    match t:
        case (True, _):
            print("true")
        case (False, 1):
            print("false one")
        case _:
            print("other")


def three_missing(t: Tuple[Bool, Bool]):
    match t:
        case (True, True):
            print("tt")


def many_missing(q: Tuple[Bool, Bool, Bool, Bool]):
    match q:
        case (True, True, True, True):
            print("tttt")


def too_complex(
    t: Tuple[Bool, Bool, Bool, Bool, Bool, Bool, Bool, Bool, Bool, Bool, Bool]
):
    match t:
        case (True, True, True, True, True, True, True, True, True, True, True):
            print("all")
        case (True, True, True, True, True, True, True, True, True, True, True):
            print("again")


def guarded_repeats(n: Int, c: Bool):
    match n:
        case 1:
            print("one")
        case 1 if c:
            print("one, guarded")
        case 2 if c:
            print("two, guarded")
        case 2:
            print("two")


def int_spellings(n: Int):
    match n:
        case 4:
            print("four")
        case 0x4:
            print("hex four")
        case 4:
            print("four again")
        case _:
            print("other int")


def float_spellings(x: Float64):
    match x:
        case 0.5:
            print("half")
        case 0.50:
            print("half again")
        case 0.5:
            print("half thrice")
        case _:
            print("other float")


def string_spellings(s: String):
    match s:
        case "a":
            print("a")
        case 'a':
            print("a again")
        case "a":
            print("a thrice")
        case _:
            print("other string")


def unreachable_arms_skip_return_analysis(b: Bool) -> Int:
    match b:
        case True:
            return 1
        case False:
            return 2
        case _:
            pass


def mid_list_unreachable_arm(o: Optional[Int]) -> Int:
    match o:
        case .Some(x):
            return x
        case .Some(y):
            pass
        case .None:
            return 0


def main():
    print(optional_returns(None), optional_returns(Optional(4)))
    print(bool_alternatives_return(False))
    print(optional_initializes(None), optional_initializes(Optional(3)))
    payload_refines(Optional(0))
    payload_refines(None)
    payload_after_whole(Optional(0))
    payload_after_whole(None)
    repeated_payload_binding(Optional(7))
    optional_and_bool(Optional(1), False)
    optional_and_bool(None, True)
    struct_product(P(False, False))
    open_leaf_product((False, 2))
    three_missing((True, True))
    many_missing((True, True, True, True))
    too_complex(
        (True, True, True, True, True, True, True, True, True, True, False)
    )
    guarded_repeats(2, False)
    int_spellings(5)
    float_spellings(0.25)
    string_spellings("b")
    print(unreachable_arms_skip_return_analysis(False))
    print(mid_list_unreachable_arm(Optional(6)), mid_list_unreachable_arm(None))
