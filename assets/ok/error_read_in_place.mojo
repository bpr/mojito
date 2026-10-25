# A read use of an `Error` borrows it where it lies, as a read use of any
# memory-only struct does: `g(e)`, `print(e)`, and a tuple element read
# copy nothing, so a later read of the same error still sees it. A
# `String` operand beside a converted literal is borrowed the same way.
def g(e: Error):
    print("g", e)


def k(e: Error):
    g(e)
    print(e)


def suffixed(s: String) -> String:
    return s + "!"


def rethrow() raises:
    try:
        raise Error("inner")
    except e:
        print("first", e)
        raise e^


def main():
    var e = Error("boom")
    g(e)
    print(e, e)
    k(e)
    var t = (Error("t0"), 2)
    g(t[0])
    print(t[0])
    print(suffixed("hi"))
    var m = String("mm")
    print(m + "!", "?" + m, m != "q")
    try:
        rethrow()
    except err:
        g(err)
        print(err)
    print(e)
