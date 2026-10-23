# A trivially copyable local stored inside a `try` part and declared outside
# it lives in registers: each raise carries its value into the handler, the
# body's completion into the `else`, and every part's completion out of the
# `try`, as Mojo's `Mem2Reg` carries it through `lit.try`'s region values.
# A store before and after a raising call, writes in the handler and the
# `else`, an accumulator carried through a loop, and a re-raise from an inner
# handler to the outer `try` each read the value the path left.
def fail(x: Int) raises:
    if x > 5:
        raise Error("big")


def try_store(c: Int) -> Int:
    var y = 0
    try:
        fail(c)
        y = c
    except:
        y = y + 100
    else:
        y = y * 2
    return y


def loop_try(n: Int) -> Int:
    var total = 0
    for i in range(n):
        try:
            total += i
            fail(i)
        except e:
            total += 1000
    return total


def nested_raise(c: Int) -> Int:
    var y = 0
    try:
        try:
            y = 1
            fail(c)
        except:
            y = y + 2
            raise Error("again")
    except:
        y = y + 10
    return y


def main():
    print(try_store(3))
    print(try_store(7))
    print(loop_try(8))
    print(nested_raise(3))
    print(nested_raise(7))
