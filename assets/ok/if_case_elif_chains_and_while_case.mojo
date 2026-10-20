# `if case`/`elif case` chains mixing pattern and boolean branches: each
# right-hand side is evaluated only when its branch is reached, a binding
# shadows an outer name in its body alone, a failed match reaches `else`;
# `while case` re-evaluates its subject at every header visit, a failed
# match runs the loop's `else`, `continue` and `break` behave as in `while`,
# and `break` skips the `else`.
def get(n: Int) -> Int:
    print("get", n)
    return n


def main():
    var x = 7
    if case (1, y) = (get(1), 2):
        print("one", y, x)
    elif case x = get(5):
        print("shadow", x)
    else:
        print("else")
    if case 9 = get(2):
        print("nine")
    elif case (a, 2) = (get(3), 2):
        print("a", a)
    else:
        print("else")
    if case 9 = get(2):
        print("nine")
    else:
        print("fell", x)
    var n = 0
    while case 0 | 1 | 2 = n:
        print("k", n)
        n += 1
        if n == 2:
            continue
        print("after", n)
    else:
        print("done", n)
    var m = 0
    while case var j = m:
        m += 1
        if j == 2:
            break
        print("j", j)
    else:
        print("not run")
    print("end", n, m)
