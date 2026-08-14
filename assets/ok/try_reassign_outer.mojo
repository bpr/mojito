# An outer variable reassigned inside a `try` body survives the block on every
# exit edge: normal completion, the caught path, a `finally` read, a
# loop-carried accumulator, escape jumps, and a nested region.
def may(x: Int) raises -> Int:
    if x < 0:
        raise Error("neg")
    return x


def main():
    var a = 0
    try:
        a = may(4)
    except e:
        pass
    print(a)

    var b = 0
    try:
        b = may(-1)
    except e:
        b = 5
    print(b)

    var c = 0
    try:
        c = may(4)
    except e:
        pass
    finally:
        print("fin", c)
    print(c)

    var acc = 0
    for i in range(3):
        try:
            acc = acc + may(i)
        except e:
            pass
    print(acc)

    var d = 0
    while True:
        try:
            d = may(7)
            break
        except e:
            break
    print(d)

    var n = 0
    try:
        try:
            n = may(4)
        except e:
            pass
    except e:
        pass
    print(n)
