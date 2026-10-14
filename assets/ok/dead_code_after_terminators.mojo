# Statements after a `return`, `break`, `continue`, or `raise` are dead: they
# lower to nothing, even when they declare a variable, read a moved value, or
# declare a nested `def`.


def take(var s: String):
    print(s)


def after_return(c: Bool) -> Int:
    var s = String("returned")
    if c:
        take(s^)
        return 1
        var dead = 5
        print(s, dead)

        def g() -> Int:
            return 41

        print(g())
    return 2


def after_break_and_continue() -> Int:
    var total = 0
    for i in range(5):
        var s = String("loop")
        if i == 1:
            take(s^)
            continue
            print(s)
        if i == 3:
            take(s^)
            break
            var dead = i * 100
            total += dead
        total += i
    return total


def after_raise(c: Bool) raises -> Int:
    var s = String("raised")
    if c:
        take(s^)
        raise Error("boom")
        var dead = String("dead")
        print(s, dead)
    return 3


def main() raises:
    print(after_return(True))
    print(after_return(False))
    print(after_break_and_continue())
    try:
        print(after_raise(True))
    except e:
        print(e)
    print(after_raise(False))
