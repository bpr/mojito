# Tuple patterns: comma subjects and patterns, a bare `1, 2` pattern, `()`
# and `(n,)`, nested tuples, and `ref`/`var` element bindings with a guard.
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
    match (1, 2):
        case 1, 2:
            print("bare tuple pattern")
    match ():
        case ():
            print("empty")
    match (4,):
        case (n,):
            print("single", n)
    match (1, (2, 3)):
        case (1, (2, y)):
            print("nested", y)
        case _:
            print("no")
    var t = (7, String("s"))
    match t:
        case (7, ref s):
            print("ref", s)
    match t:
        case (var n, s) if n > 5:
            n += 1
            print("guarded", n, s, t[0])
    print("end")
