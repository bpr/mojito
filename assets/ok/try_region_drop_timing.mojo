# Per-instruction drop elaboration inside `try` regions: rebinds in the body,
# handler, `else`, and `finally` run the overwritten value's destructor; body
# locals, escapes crossing the region, and nested regions drop exactly once.
struct Noisy(Deinitable, Movable, Copyable):
    var tag: Int

    def __init__(out self, tag: Int):
        self.tag = tag

    def __deinit__(deinit self):
        print("deinit", self.tag)


def may(x: Int) raises -> Int:
    if x < 0:
        raise Error("neg")
    return x


def make(x: Int) raises -> Noisy:
    if x < 0:
        raise Error("neg")
    return Noisy(x)


def regions() raises:
    var a = Noisy(1)
    try:
        a = make(2)
    except e:
        a = Noisy(3)
    else:
        a = Noisy(4)
    finally:
        var local = Noisy(5)
        print("fin", local.tag)
    print("post", a.tag)


def escapes():
    var total = 0
    for i in range(4):
        var outer = Noisy(10 + i)
        try:
            var inner = Noisy(20 + i)
            print("use", outer.tag, inner.tag)
            if i == 2:
                break
            total = may(i)
        except e:
            print("caught")
        finally:
            print("fin", i)
    print("total", total)


def nested():
    var n = Noisy(1)
    try:
        try:
            n = make(7)
        except e:
            print("inner")
    except e:
        print("outer")
    print("post", n.tag)


def main():
    try:
        regions()
    except e:
        print("caught top")
    escapes()
    nested()
