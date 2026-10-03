# A value live after a region and unused in the arm that raises is never
# destroyed when the raise leaves the function. The pin prints `del b<n>` on
# every path here; Mojito prints it only for `b4`, which is dead at the
# raise. Filed as docs/roadmap.md 3.1 from the compile-time region
# ownership probes (docs/notes/comptime-region-ownership.md).
struct Thing(Movable):
    var s: String

    def __init__(out self, var s: String):
        self.s = s^

    def __del__(deinit self):
        print("del", self.s)

def consume(var t: Thing):
    print("consume", t.s)

def look(t: Thing):
    print("look", t.s)

def fail() raises:
    raise Error("boom")

def v3(cond: Bool) raises:
    var b = Thing(String("b3"))
    if cond:
        look(b)
        raise Error("boom")
    print("after", b.s)

def v4(cond: Bool) raises:
    var b = Thing(String("b4"))
    if cond:
        raise Error("boom")
    print("after")

def v5(cond: Bool) raises:
    var b = Thing(String("b5"))
    var i = 0
    while i < 2:
        if cond:
            raise Error("boom")
        i += 1
    print("after", b.s)

def v6(cond: Bool) raises:
    var b = Thing(String("b6"))
    if cond:
        fail()
    print("after", b.s)

def v7(cond: Bool) raises:
    var b = Thing(String("b7"))
    if cond:
        print("then")
    else:
        raise Error("boom")
    print("after", b.s)

def main():
    try:
        v3(True)
    except e:
        print("caught", e)
    try:
        v4(True)
    except e:
        print("caught", e)
    try:
        v5(True)
    except e:
        print("caught", e)
    try:
        v6(True)
    except e:
        print("caught", e)
    try:
        v7(False)
    except e:
        print("caught", e)
