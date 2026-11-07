# An `if` or `comptime if` with an `else` whose every arm leaves ends its
# block, so the code after it is dead and erased before an instance judges
# its assertions, as at the pin; an assertion after a `comptime if` with
# no `else` is judged, and holds here.


def both_return[n: Int]() -> Int:
    comptime if n == 1:
        return 1
    else:
        return 2
    comptime assert n == 5, "dead"


def assert_else[n: Int]() -> Int:
    comptime if n == 1:
        return 1
    else:
        comptime assert False, "else"
    comptime assert n == 5, "dead"
    return 3


def runtime_if[n: Int](x: Int) raises -> Int:
    if x > 0:
        return 1
    else:
        raise Error("e")
    comptime assert n == 5, "dead"


def loop_exits[n: Int]() -> Int:
    var count = 0
    for _ in range(3):
        count += 1
        comptime if n == 1:
            break
        else:
            continue
        comptime assert n == 5, "dead"
    return count


def holds[n: Int]() -> Int:
    comptime if n == 1:
        return 1
    comptime assert n == 1 or n == 2, "judged"
    return 3


def main() raises:
    print(both_return[1](), both_return[2]())
    print(assert_else[1]())
    print(runtime_if[1](1))
    print(loop_exits[1](), loop_exits[2]())
    print(holds[1](), holds[2]())
