# Comma subjects and patterns, `case Int` binding, `== -3` for a signed
# literal, no-match fallthrough, a guarded literal, `()` and `(n,)` tuples,
# `1 | 2` alternatives, and a float literal.
def main():
    var a = True
    var b = False
    match a, b:
        case True, True:
            print("tt")
        case True, _:
            print("t_")
        case _:
            print("other")
    match 5:
        case 1 | 2:
            print("low")
        case Int:
            print("bound", Int)
    match -3:
        case == -3:
            print("neg")
        case _:
            print("pos")
    match 2:
        case 1:
            print("one")
    print("fallthrough")
    match "x":
        case "x" if True:
            print("guarded")
    match (1, 2):
        case 1, 2:
            print("bare tuple pattern")
    match ():
        case ():
            print("empty")
    match (4,):
        case (n,):
            print("single", n)
    var f = 2.5
    match f:
        case 2.5:
            print("float")
        case _:
            print("nf")
