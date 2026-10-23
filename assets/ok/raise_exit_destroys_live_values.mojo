# A `raise` statement ends its region, as upstream's `raise` ends its block,
# so a value live after the region and unread on the raise path is destroyed
# at the raising arm's entry, whether the raise leaves the function or a
# `try` body, in an `if`, `else`, loop, or `comptime if` arm. The pin prints
# each `del` before the arm's own output.
struct Thing(Movable):
    var s: String

    def __init__(out self, var s: String):
        self.s = s^

    def __deinit__(deinit self):
        print("del", self.s)


def leaves_function(cond: Bool) raises:
    var b = Thing(String("b1"))
    var c = Thing(String("c1"))
    if cond:
        print("arm")
        print("look", c.s)
        raise Error("boom")
    print("after", b.s, c.s)


def leaves_try_body(cond: Bool) raises:
    var b = Thing(String("b2"))
    try:
        if cond:
            print("arm")
            raise Error("boom")
        print("after", b.s)
    except e:
        print("caught", e)
    print("end")


def handler_reads_outer(cond: Bool) raises:
    var b = Thing(String("b3"))
    var d = Thing(String("d3"))
    try:
        var c = Thing(String("c3"))
        if cond:
            print("arm")
            raise Error("boom")
        print("after", b.s, c.s)
    except e:
        print("caught", e, d.s)
    print("end")


def in_else_arm(cond: Bool) raises:
    var b = Thing(String("e4"))
    if cond:
        print("then")
    else:
        print("else")
        raise Error("boom")
    print("after", b.s)


def in_loop(n: Int) raises:
    var b = Thing(String("l5"))
    for i in range(n):
        if i == 1:
            raise Error("boom")
    print("after", b.s)


def in_comptime_arm[flag: Bool]() raises:
    var b = Thing(String("c6"))
    comptime if flag:
        raise Error("boom")
    print("after", b.s)


def main() raises:
    try:
        leaves_function(True)
    except e:
        print("caught", e)
    leaves_try_body(True)
    handler_reads_outer(True)
    try:
        in_else_arm(False)
    except e:
        print("caught", e)
    try:
        in_loop(3)
    except e:
        print("caught", e)
    try:
        in_comptime_arm[True]()
    except e:
        print("caught", e)
    in_comptime_arm[False]()
