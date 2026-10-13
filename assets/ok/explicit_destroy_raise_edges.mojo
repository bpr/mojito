# Raise edges of values that must be explicitly destroyed: an `except` arm
# starts from the state its raise points reach it with, an arm no raise point
# reaches is unreachable, and a raise reaching `finally` runs it first. Every
# routine here consumes its value on each raise edge, as the pinned Mojo
# requires, and the pin accepts each one.
@explicit_destroy("L must be closed")
struct L(Movable, Deinitable where False):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def close(deinit self):
        print("close", self.v)

def take(var a: L, n: Int):
    a^.close()

def look(a: L) -> Int:
    return a.v

def boom(n: Int) raises -> Int:
    if n > 5:
        raise Error("boom " + String(n))
    return n

def nob(n: Int) -> Int:
    return n

def take_raising(var a: L, n: Int) raises:
    a^.close()
    _ = boom(n)

# A `raise` inside a `try` reaches its `except` arm, not the function's exit:
# the outer `l` flows into the arm and is closed after the statement.
def raise_into_handler():
    var l = L(1)
    try:
        raise Error("raised")
    except e:
        print(e)
    l^.close()

# The arm sees `l` as the raising call left it: still owned, so the arm closes
# it, while the normal route closes it after the call.
def handler_closes_after_raise(n: Int):
    var l = L(2)
    try:
        _ = boom(n)
        l^.close()
    except e:
        print(e)
        l^.close()

# Nothing in the `try` body raises, so the arm is unreachable and its close
# does not make `l`'s later close conditional.
def unreachable_handler():
    var l = L(3)
    try:
        print(nob(30))
    except e:
        l^.close()
    l^.close()

# A raise reaching `finally` runs it before leaving the function, and the
# `finally` body closes `l` on every route.
def finally_closes(n: Int) raises:
    var l = L(4)
    try:
        _ = boom(n)
    except e:
        _ = boom(n + 1)
    finally:
        l^.close()

# A raising callee has consumed its owned argument before it raises.
def callee_consumed_before_raise(n: Int):
    var l = L(5)
    try:
        take_raising(l^, n)
    except e:
        print(e)

# An argument that reads `l` after `l^` runs before the call consumes `l`, and
# an outer value survives a handled raise.
def read_after_transfer(n: Int):
    var l = L(6)
    take(l^, look(l))
    var m = L(7)
    try:
        _ = boom(n)
    except e:
        print(e)
    m^.close()

def main() raises:
    raise_into_handler()
    handler_closes_after_raise(1)
    handler_closes_after_raise(9)
    unreachable_handler()
    finally_closes(1)
    try:
        finally_closes(6)
    except e:
        print(e)
    callee_consumed_before_raise(1)
    callee_consumed_before_raise(9)
    read_after_transfer(1)
    read_after_transfer(9)
