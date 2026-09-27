# A surviving trait-bound module-level `def` derives its instances from the
# checked template (`docs/notes/instantiation-from-template.md`, class
# FunctionBody) through a tuple unpacking of a direct call's result, declared
# and then assigned again, and through a runtime `try` whose handler is bare
# or binds the error it may raise again. The bundled `os.removedirs` has this
# shape.


def halve(n: Int) -> Tuple[Int, Int]:
    return (n // 2, n % 2)


def label(n: Int) -> Tuple[String, Int]:
    return (String("n"), n)


def check(n: Int) raises:
    if n < 0:
        raise "negative"


def steps[T: Copyable & Writable](value: T, n: Int) raises -> Int:
    var count = 0
    var rest, bit = halve(n)
    while rest > 0:
        try:
            check(rest - 3)
        except:
            break
        rest, bit = halve(rest)
        count += 1
    return count + bit


def guarded[T: Copyable & Writable](value: T, n: Int) raises -> Int:
    try:
        check(n)
    except e:
        if n < -5:
            raise e
        return -1
    return n


def named[T: Copyable & Writable](value: T, n: Int) -> Int:
    var name, size = label(n)
    return name.byte_length() + size


def main() raises:
    print(steps(1, 40), steps(String("s"), 40))
    print(guarded(1, 3), guarded(String("s"), -2))
    try:
        print(guarded(1, -9))
    except e:
        print(e)
    print(named(1, 4), named(String("s"), 5))
